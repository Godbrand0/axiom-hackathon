//! Axiom Pay: a marketplace where AI agents sell research to other agents, inside budgets humans control.
//!
//! * **Wallets.** Any human (owner) opens a wallet for its agents: a budget, a per-call cap, a daily cap, and an
//!   allowlist of services at approved prices. One wallet can fund a fleet of agents (`add_agent`); they share
//!   the budget and caps, and can spend only inside them.
//! * **Services.** Any account can sell a service at a price per call. Payment goes straight to the seller.
//!   If the seller changes the price, existing approvals stop matching and buyers cannot pay until their
//!   owner approves the new price.
//! * **Receipts.** Each payment stores a receipt bound to a hash of the exact request. The seller checks the
//!   receipt (a free view call) before serving.
//! * **Commitments.** Any account can commit the hash of a prediction (for Axiom: a trade signal). The block
//!   time proves it existed before its outcome. Buyers use a seller's commitments to verify its track record
//!   before paying for new signals.
//! * **Jobs (swarm bidding).** A buyer agent posts a job with a maximum price; seller agents bid; the buyer
//!   awards one bid and pays it from its wallet under the same caps. The buyer then confirms delivery or
//!   disputes it, which builds each seller's on-chain delivery record.
//! * **Pools (collective purchase).** Several agents split one purchase. Each share is debited from its own
//!   wallet into escrow; when the last share arrives the seller is paid and every member may redeem the
//!   receipt. An unfilled pool is refunded to the wallets after its deadline.
//! * **Storage credit.** Agents that hold only a restricted function-call key cannot attach NEAR to calls, so an
//!   owner pre-funds their storage (`fund_storage`) and the contract draws on it when no deposit is attached.
use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, LookupMap, Vector};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

const NANOS_PER_DAY: u64 = 86_400 * 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * 1_000_000_000;
const MAX_ID_LEN: usize = 64;
const MAX_TEXT_LEN: usize = 280;
const MIN_STORAGE_FUNDING: u128 = 10_000_000_000_000_000_000_000; // 0.01 NEAR
const MAX_POOL_SHARES: u32 = 10;
const MAX_BIDS_PER_JOB: usize = 20;

/// Storage prefixes. New variants are only ever appended, so existing data keeps its prefix across upgrades.
#[near]
#[derive(BorshStorageKey)]
enum Key {
    Wallets,
    AgentOwner,
    Approvals,
    Services,
    Receipts,
    Commitments,
    CommitmentIndex,
    // v3
    Credits,
    OwnerAgents,
    Jobs,
    Bids,
    SellerStats,
    Pools,
    PoolOfReceipt,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Wallet {
    pub owner: AccountId,
    /// The wallet's first agent. More can be added with `add_agent`.
    pub agent: AccountId,
    pub max_per_call: U128,
    pub daily_cap: U128,
    /// Deposited funds not yet spent or withdrawn.
    pub budget: U128,
    pub spent_day: u64,
    pub spent_today: U128,
    pub payments: u64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Service {
    /// Seller; receives every payment.
    pub provider: AccountId,
    /// Price per call in yoctoNEAR.
    pub price: U128,
    pub description: String,
    pub calls: u64,
    pub earned: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Receipt {
    pub id: u64,
    /// The service paid for; `job:<id>` for an awarded job.
    pub service_id: String,
    pub provider: AccountId,
    /// Agent that paid, and the owner whose wallet funded it. For a pool: its first member.
    pub payer: AccountId,
    pub owner: AccountId,
    pub amount: U128,
    /// Hex SHA-256 of the request being paid for. The seller serves only a matching request.
    pub request_hash: String,
    pub memo: String,
    pub timestamp_ns: u64,
    pub block_height: u64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Commitment {
    pub id: u64,
    pub committer: AccountId,
    /// Hex SHA-256 of the canonical prediction record.
    pub hash: String,
    pub label: String,
    pub timestamp_ns: u64,
    pub block_height: u64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Debug)]
pub enum JobStatus {
    Open,
    Awarded,
    Delivered,
    Disputed,
    Cancelled,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Job {
    pub id: u64,
    /// Agent that posted the job and will pay for it, and its wallet's owner.
    pub buyer: AccountId,
    pub owner: AccountId,
    /// What is wanted, in a form sellers understand, e.g. `sec-red-flags|JPM|2026-10-10`.
    pub topic: String,
    pub max_price: U128,
    pub deadline_ns: u64,
    pub status: JobStatus,
    pub winner: Option<AccountId>,
    pub price: U128,
    pub receipt: Option<u64>,
    pub bids: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Bid {
    pub seller: AccountId,
    pub price: U128,
    /// Where the seller delivers: `GET <endpoint>/v1/jobs/<id>?receipt=<n>`.
    pub endpoint: String,
    pub timestamp_ns: u64,
}

/// A seller's on-chain delivery record from jobs.
#[near(serializers = [borsh, json])]
#[derive(Clone, Default)]
pub struct SellerStats {
    pub awarded: u64,
    pub delivered: u64,
    pub disputed: u64,
    pub earned: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Debug)]
pub enum PoolStatus {
    Open,
    Filled,
    Refunded,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Pool {
    pub id: u64,
    pub service_id: String,
    pub provider: AccountId,
    pub request_hash: String,
    pub memo: String,
    /// Total price, fixed when the pool opened.
    pub price: U128,
    pub shares: u32,
    /// Member agents, their wallet owners, and what each paid into escrow.
    pub members: Vec<AccountId>,
    pub owners: Vec<AccountId>,
    pub paid: Vec<U128>,
    pub deadline_ns: u64,
    pub status: PoolStatus,
    pub receipt: Option<u64>,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct AxiomPay {
    wallets: LookupMap<AccountId, Wallet>,
    /// agent -> owner of the wallet it spends from.
    agent_owner: LookupMap<AccountId, AccountId>,
    /// "owner|service_id" -> approved price.
    approvals: LookupMap<String, u128>,
    services: IterableMap<String, Service>,
    receipts: Vector<Receipt>,
    commitments: Vector<Commitment>,
    /// "committer|hash" -> commitment id.
    commitment_index: LookupMap<String, u64>,
    // v3
    /// Prepaid storage per account, used when a call attaches no deposit.
    credits: LookupMap<AccountId, u128>,
    /// owner -> agents added after the wallet's first agent.
    owner_agents: LookupMap<AccountId, Vec<AccountId>>,
    jobs: Vector<Job>,
    bids: LookupMap<u64, Vec<Bid>>,
    seller_stats: LookupMap<AccountId, SellerStats>,
    pools: Vector<Pool>,
    /// receipt id -> pool id, for receipts that settled a pool.
    pool_of_receipt: LookupMap<u64, u64>,
}

/// State layout of v2, read once by `migrate`.
#[near(serializers = [borsh])]
pub struct AxiomPayV2 {
    wallets: LookupMap<AccountId, Wallet>,
    agent_owner: LookupMap<AccountId, AccountId>,
    approvals: LookupMap<String, u128>,
    services: IterableMap<String, Service>,
    receipts: Vector<Receipt>,
    commitments: Vector<Commitment>,
    commitment_index: LookupMap<String, u64>,
}

fn require_hash(h: &str) {
    require!(
        h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "hash must be 64 lowercase hex chars"
    );
}

fn require_text(s: &str, max: usize, what: &str) {
    require!(s.len() <= max, format!("{what} is longer than {max} bytes"));
}

fn approval_key(owner: &AccountId, service_id: &str) -> String {
    format!("{owner}|{service_id}")
}

fn today() -> u64 {
    env::block_timestamp() / NANOS_PER_DAY
}

fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    bytes.iter().flat_map(|b| [D[(b >> 4) as usize] as char, D[(b & 15) as usize] as char]).collect()
}

/// The request a job's receipt is bound to: SHA-256 of `axiom-job/v1|<id>|<topic>`.
pub fn job_request_hash(id: u64, topic: &str) -> String {
    hex(&env::sha256(format!("axiom-job/v1|{id}|{topic}").as_bytes()))
}

fn ttl_deadline(ttl_minutes: u32) -> u64 {
    require!((1..=7 * 24 * 60).contains(&ttl_minutes), "ttl_minutes must be between 1 and 10080");
    env::block_timestamp() + ttl_minutes as u64 * NANOS_PER_MINUTE
}

#[near]
impl AxiomPay {
    #[init]
    pub fn new() -> Self {
        Self::from_v2(AxiomPayV2 {
            wallets: LookupMap::new(Key::Wallets),
            agent_owner: LookupMap::new(Key::AgentOwner),
            approvals: LookupMap::new(Key::Approvals),
            services: IterableMap::new(Key::Services),
            receipts: Vector::new(Key::Receipts),
            commitments: Vector::new(Key::Commitments),
            commitment_index: LookupMap::new(Key::CommitmentIndex),
        })
    }

    /// Upgrades v2 state in place (wallets, receipts and commitments are kept). Only the contract account can call it.
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let old: AxiomPayV2 = env::state_read().unwrap_or_else(|| env::panic_str("no v2 state to migrate"));
        Self::from_v2(old)
    }

    // ---------- owners (humans) ----------

    /// Opens the caller's wallet for `agent`. The deposit pays the wallet's storage; the rest is budget.
    #[payable]
    pub fn open_wallet(&mut self, agent: AccountId, max_per_call: U128, daily_cap: U128) -> Wallet {
        let owner = env::predecessor_account_id();
        require!(!self.wallets.contains_key(&owner), "wallet already open");
        require!(!self.agent_owner.contains_key(&agent), "agent already spends from another wallet");
        let before = env::storage_usage();
        let w = Wallet {
            owner: owner.clone(),
            agent: agent.clone(),
            max_per_call,
            daily_cap,
            budget: U128(0),
            spent_day: 0,
            spent_today: U128(0),
            payments: 0,
        };
        self.wallets.insert(owner.clone(), w);
        self.agent_owner.insert(agent, owner.clone());
        self.wallets.flush();
        self.agent_owner.flush();
        let rest = self.charge_storage(before, env::attached_deposit().as_yoctonear());
        let w = self.wallets.get_mut(&owner).unwrap();
        w.budget = U128(rest);
        w.clone()
    }

    /// Adds the attached deposit to the caller's budget.
    #[payable]
    pub fn deposit(&mut self) -> U128 {
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount > 0, "attach a deposit");
        let w = self.wallet_mut(&env::predecessor_account_id());
        w.budget = U128(w.budget.0 + amount);
        w.budget
    }

    pub fn withdraw(&mut self, amount: U128) -> Promise {
        let owner = env::predecessor_account_id();
        let w = self.wallet_mut(&owner);
        require!(amount.0 <= w.budget.0, "amount exceeds budget");
        w.budget = U128(w.budget.0 - amount.0);
        Promise::new(owner).transfer(NearToken::from_yoctonear(amount.0))
    }

    pub fn set_policy(&mut self, max_per_call: U128, daily_cap: U128) {
        let w = self.wallet_mut(&env::predecessor_account_id());
        w.max_per_call = max_per_call;
        w.daily_cap = daily_cap;
    }

    /// Replaces the wallet's first agent.
    pub fn set_agent(&mut self, agent: AccountId) {
        let owner = env::predecessor_account_id();
        require!(!self.agent_owner.contains_key(&agent), "agent already spends from another wallet");
        let old = self.wallet_mut(&owner).agent.clone();
        self.agent_owner.remove(&old);
        self.agent_owner.insert(agent.clone(), owner.clone());
        self.wallet_mut(&owner).agent = agent;
    }

    /// Lets one more agent spend from the caller's wallet, under the same budget, caps and approvals.
    #[payable]
    pub fn add_agent(&mut self, agent: AccountId) {
        let owner = env::predecessor_account_id();
        self.wallet_mut(&owner);
        require!(!self.agent_owner.contains_key(&agent), "agent already spends from another wallet");
        let before = env::storage_usage();
        self.agent_owner.insert(agent.clone(), owner.clone());
        let mut list = self.owner_agents.get(&owner).cloned().unwrap_or_default();
        list.push(agent);
        self.owner_agents.insert(owner, list);
        self.agent_owner.flush();
        self.owner_agents.flush();
        self.charge_storage_refund(before);
    }

    /// Stops an agent added with `add_agent` from spending.
    pub fn remove_agent(&mut self, agent: AccountId) {
        let owner = env::predecessor_account_id();
        let mut list = self.owner_agents.get(&owner).cloned().unwrap_or_default();
        let before = list.len();
        list.retain(|a| a != &agent);
        require!(list.len() < before, "not an added agent of your wallet (use set_agent for the first agent)");
        self.owner_agents.insert(owner, list);
        self.agent_owner.remove(&agent);
    }

    /// Allowlists a service for the caller's agents at `price`, which must be the current price.
    pub fn approve_service(&mut self, service_id: String, price: U128) {
        let owner = env::predecessor_account_id();
        self.wallet_mut(&owner);
        let svc = self.services.get(&service_id).unwrap_or_else(|| env::panic_str("unknown service"));
        require!(svc.price == price, "price changed since you looked; re-check the service");
        self.approvals.insert(approval_key(&owner, &service_id), price.0);
    }

    pub fn revoke_service(&mut self, service_id: String) {
        self.approvals.remove(&approval_key(&env::predecessor_account_id(), &service_id));
    }

    /// Prepays storage for `account` (default: the caller), for agents whose key cannot attach deposits.
    #[payable]
    pub fn fund_storage(&mut self, account: Option<AccountId>) -> U128 {
        let account = account.unwrap_or_else(env::predecessor_account_id);
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount >= MIN_STORAGE_FUNDING, "attach at least 0.01 NEAR");
        let total = self.credits.get(&account).copied().unwrap_or(0) + amount;
        self.credits.insert(account, total);
        U128(total)
    }

    // ---------- sellers ----------

    /// Registers or reprices a service owned by the caller. The caller pays the storage it uses.
    #[payable]
    pub fn register_service(&mut self, service_id: String, price: U128, description: String) {
        require!(
            !service_id.is_empty()
                && service_id.len() <= MAX_ID_LEN
                && !service_id.contains('|')
                && !service_id.starts_with("job:"),
            "bad service_id"
        );
        require_text(&description, MAX_TEXT_LEN, "description");
        let provider = env::predecessor_account_id();
        let before = env::storage_usage();
        let svc = match self.services.get(&service_id) {
            Some(old) => {
                require!(old.provider == provider, "service_id belongs to another provider");
                Service { price, description, ..old.clone() }
            }
            None => Service { provider, price, description, calls: 0, earned: U128(0) },
        };
        self.services.insert(service_id, svc);
        self.services.flush();
        self.charge_storage_refund(before);
    }

    /// Bids on an open job (or lowers the caller's earlier bid). The caller pays the bid's storage.
    #[payable]
    pub fn bid(&mut self, job_id: u64, price: U128, endpoint: String) {
        let seller = env::predecessor_account_id();
        require_text(&endpoint, 200, "endpoint");
        let job = self.job(job_id).clone();
        require!(job.status == JobStatus::Open, "job is not open");
        require!(env::block_timestamp() < job.deadline_ns, "job has expired");
        require!(price.0 > 0 && price.0 <= job.max_price.0, "price must be above 0 and at most the job's max_price");
        require!(seller != job.buyer && seller != job.owner, "cannot bid on your own job");
        let before = env::storage_usage();
        let mut bids = self.bids.get(&job_id).cloned().unwrap_or_default();
        let b = Bid { seller: seller.clone(), price, endpoint, timestamp_ns: env::block_timestamp() };
        match bids.iter_mut().find(|x| x.seller == seller) {
            Some(old) => *old = b,
            None => {
                require!(bids.len() < MAX_BIDS_PER_JOB, "job has the maximum number of bids");
                bids.push(b);
                self.jobs.get_mut(job_id as u32).unwrap().bids += 1;
            }
        }
        self.bids.insert(job_id, bids);
        self.bids.flush();
        self.jobs.flush();
        self.charge_storage_refund(before);
    }

    /// Records a prediction hash for the caller, who pays its storage. The first commitment's time counts.
    #[payable]
    pub fn commit(&mut self, hash: String, label: String) -> Commitment {
        require_hash(&hash);
        require_text(&label, MAX_TEXT_LEN, "label");
        let committer = env::predecessor_account_id();
        let key = format!("{committer}|{hash}");
        require!(!self.commitment_index.contains_key(&key), "hash already committed");
        let before = env::storage_usage();
        let c = Commitment {
            id: self.commitments.len() as u64,
            committer,
            hash,
            label,
            timestamp_ns: env::block_timestamp(),
            block_height: env::block_height(),
        };
        self.commitments.push(c.clone());
        self.commitment_index.insert(key, c.id);
        self.commitments.flush();
        self.commitment_index.flush();
        self.charge_storage_refund(before);
        c
    }

    // ---------- agents (buyers) ----------

    /// Pays one call of `service_id` from the caller's wallet. Enforces the owner's approval at the current
    /// price, the per-call cap, the daily cap and the budget, then pays the seller.
    pub fn pay(&mut self, service_id: String, request_hash: String, memo: String) -> Receipt {
        let agent = env::predecessor_account_id();
        require_hash(&request_hash);
        require_text(&memo, MAX_TEXT_LEN, "memo");
        let owner = self.owner_of(&agent);
        let svc = self.services.get(&service_id).unwrap_or_else(|| env::panic_str("unknown service")).clone();
        self.require_approved(&owner, &service_id, svc.price.0);
        self.debit(&owner, svc.price.0);
        let s = self.services.get_mut(&service_id).unwrap();
        s.calls += 1;
        s.earned = U128(s.earned.0 + svc.price.0);
        self.settle(service_id, svc.provider, agent, owner, svc.price.0, request_hash, memo)
    }

    /// Posts a job for sellers to bid on. `max_price` must fit the wallet's per-call cap.
    #[payable]
    pub fn post_job(&mut self, topic: String, max_price: U128, ttl_minutes: u32) -> Job {
        let buyer = env::predecessor_account_id();
        require!(!topic.is_empty(), "empty topic");
        require_text(&topic, MAX_TEXT_LEN, "topic");
        let owner = self.owner_of(&buyer);
        require!(max_price.0 > 0, "max_price must be above 0");
        require!(max_price.0 <= self.wallets.get(&owner).unwrap().max_per_call.0, "max_price exceeds max_per_call");
        let before = env::storage_usage();
        let job = Job {
            id: self.jobs.len() as u64,
            buyer,
            owner,
            topic,
            max_price,
            deadline_ns: ttl_deadline(ttl_minutes),
            status: JobStatus::Open,
            winner: None,
            price: U128(0),
            receipt: None,
            bids: 0,
        };
        self.jobs.push(job.clone());
        self.jobs.flush();
        self.charge_storage_refund(before);
        job
    }

    /// Awards a job to `seller`'s bid and pays it from the caller's wallet (per-call cap, daily cap, budget).
    pub fn award(&mut self, job_id: u64, seller: AccountId) -> Receipt {
        let buyer = env::predecessor_account_id();
        let job = self.job(job_id).clone();
        require!(job.buyer == buyer, "only the agent that posted the job can award it");
        require!(job.status == JobStatus::Open, "job is not open");
        let bid = self
            .bids
            .get(&job_id)
            .and_then(|bs| bs.iter().find(|b| b.seller == seller).cloned())
            .unwrap_or_else(|| env::panic_str("no bid from that seller"));
        let owner = self.owner_of(&buyer);
        self.debit(&owner, bid.price.0);
        let receipt = self.settle(
            format!("job:{job_id}"),
            seller.clone(),
            buyer,
            owner,
            bid.price.0,
            job_request_hash(job_id, &job.topic),
            job.topic.chars().take(MAX_TEXT_LEN).collect(),
        );
        let j = self.jobs.get_mut(job_id as u32).unwrap();
        j.status = JobStatus::Awarded;
        j.winner = Some(seller.clone());
        j.price = bid.price;
        j.receipt = Some(receipt.id);
        let mut st = self.seller_stats.get(&seller).cloned().unwrap_or_default();
        st.awarded += 1;
        st.earned = U128(st.earned.0 + bid.price.0);
        self.seller_stats.insert(seller, st);
        receipt
    }

    /// The buyer (or its owner) records whether the awarded seller delivered. This is the seller's reputation.
    pub fn confirm(&mut self, job_id: u64, delivered: bool) {
        let caller = env::predecessor_account_id();
        let job = self.job(job_id).clone();
        require!(caller == job.buyer || caller == job.owner, "only the buyer or its owner can confirm");
        require!(job.status == JobStatus::Awarded, "job is not awaiting confirmation");
        let seller = job.winner.unwrap();
        let mut st = self.seller_stats.get(&seller).cloned().unwrap_or_default();
        if delivered {
            st.delivered += 1;
        } else {
            st.disputed += 1;
        }
        self.seller_stats.insert(seller, st);
        self.jobs.get_mut(job_id as u32).unwrap().status = if delivered { JobStatus::Delivered } else { JobStatus::Disputed };
    }

    pub fn cancel_job(&mut self, job_id: u64) {
        let caller = env::predecessor_account_id();
        let job = self.job(job_id);
        require!(caller == job.buyer || caller == job.owner, "only the buyer or its owner can cancel");
        require!(job.status == JobStatus::Open, "job is not open");
        self.jobs.get_mut(job_id as u32).unwrap().status = JobStatus::Cancelled;
    }

    /// Opens a pool to split one call of `service_id` between `shares` agents, and pays the caller's share.
    #[payable]
    pub fn open_pool(&mut self, service_id: String, request_hash: String, memo: String, shares: u32, ttl_minutes: u32) -> Pool {
        require_hash(&request_hash);
        require_text(&memo, MAX_TEXT_LEN, "memo");
        require!((2..=MAX_POOL_SHARES).contains(&shares), "shares must be between 2 and 10");
        let svc = self.services.get(&service_id).unwrap_or_else(|| env::panic_str("unknown service")).clone();
        require!(svc.price.0 >= shares as u128, "price too small to split");
        let before = env::storage_usage();
        let pool = Pool {
            id: self.pools.len() as u64,
            service_id,
            provider: svc.provider,
            request_hash,
            memo,
            price: svc.price,
            shares,
            members: vec![],
            owners: vec![],
            paid: vec![],
            deadline_ns: ttl_deadline(ttl_minutes),
            status: PoolStatus::Open,
            receipt: None,
        };
        let id = pool.id;
        self.pools.push(pool);
        self.pools.flush();
        self.join(id, before)
    }

    /// Pays the caller's share of an open pool. The last share settles the purchase with the seller.
    #[payable]
    pub fn join_pool(&mut self, pool_id: u64) -> Pool {
        self.join(pool_id, env::storage_usage())
    }

    /// After the deadline, returns every share of an unfilled pool to its wallet. Anyone can call it.
    pub fn refund_pool(&mut self, pool_id: u64) -> Pool {
        let pool = self.pool(pool_id).clone();
        require!(pool.status == PoolStatus::Open, "pool is not open");
        require!(env::block_timestamp() >= pool.deadline_ns, "pool has not expired yet");
        for (owner, paid) in pool.owners.iter().zip(&pool.paid) {
            if let Some(w) = self.wallets.get_mut(owner) {
                w.budget = U128(w.budget.0 + paid.0);
            }
        }
        let p = self.pools.get_mut(pool_id as u32).unwrap();
        p.status = PoolStatus::Refunded;
        p.clone()
    }

    // ---------- views ----------

    /// The owner's wallet, with `spent_today` reset if the day has rolled over.
    pub fn get_wallet(&self, owner: AccountId) -> Option<Wallet> {
        self.wallets.get(&owner).map(|w| {
            let mut w = w.clone();
            if w.spent_day != today() {
                w.spent_today = U128(0);
            }
            w
        })
    }

    pub fn get_wallet_of_agent(&self, agent: AccountId) -> Option<Wallet> {
        self.get_wallet(self.agent_owner.get(&agent)?.clone())
    }

    /// Every agent allowed to spend from `owner`'s wallet.
    pub fn get_agents(&self, owner: AccountId) -> Vec<AccountId> {
        let Some(w) = self.wallets.get(&owner) else { return vec![] };
        let mut v = vec![w.agent.clone()];
        v.extend(self.owner_agents.get(&owner).cloned().unwrap_or_default());
        v
    }

    /// The price `owner` approved for `service_id`, if any.
    pub fn get_approval(&self, owner: AccountId, service_id: String) -> Option<U128> {
        self.approvals.get(&approval_key(&owner, &service_id)).map(|p| U128(*p))
    }

    pub fn get_storage_credit(&self, account: AccountId) -> U128 {
        U128(self.credits.get(&account).copied().unwrap_or(0))
    }

    pub fn get_service(&self, service_id: String) -> Option<Service> {
        self.services.get(&service_id).cloned()
    }

    pub fn list_services(&self) -> Vec<(String, Service)> {
        self.services.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub fn get_receipt(&self, id: u64) -> Option<Receipt> {
        self.receipts.get(id as u32).cloned()
    }

    pub fn receipt_count(&self) -> u64 {
        self.receipts.len() as u64
    }

    /// Receipts `[from, from+limit)`.
    pub fn get_receipts(&self, from: u64, limit: u64) -> Vec<Receipt> {
        (from..from.saturating_add(limit).min(self.receipts.len() as u64))
            .filter_map(|i| self.receipts.get(i as u32).cloned())
            .collect()
    }

    pub fn commitment_count(&self) -> u64 {
        self.commitments.len() as u64
    }

    pub fn get_commitments(&self, from: u64, limit: u64) -> Vec<Commitment> {
        (from..from.saturating_add(limit).min(self.commitments.len() as u64))
            .filter_map(|i| self.commitments.get(i as u32).cloned())
            .collect()
    }

    pub fn find_commitment(&self, committer: AccountId, hash: String) -> Option<Commitment> {
        let id = *self.commitment_index.get(&format!("{committer}|{hash}"))?;
        self.commitments.get(id as u32).cloned()
    }

    pub fn job_count(&self) -> u64 {
        self.jobs.len() as u64
    }

    pub fn get_job(&self, id: u64) -> Option<Job> {
        self.jobs.get(id as u32).cloned()
    }

    pub fn get_jobs(&self, from: u64, limit: u64) -> Vec<Job> {
        (from..from.saturating_add(limit).min(self.jobs.len() as u64)).filter_map(|i| self.jobs.get(i as u32).cloned()).collect()
    }

    pub fn get_bids(&self, job_id: u64) -> Vec<Bid> {
        self.bids.get(&job_id).cloned().unwrap_or_default()
    }

    pub fn get_seller_stats(&self, seller: AccountId) -> SellerStats {
        self.seller_stats.get(&seller).cloned().unwrap_or_default()
    }

    pub fn pool_count(&self) -> u64 {
        self.pools.len() as u64
    }

    pub fn get_pool(&self, id: u64) -> Option<Pool> {
        self.pools.get(id as u32).cloned()
    }

    pub fn get_pools(&self, from: u64, limit: u64) -> Vec<Pool> {
        (from..from.saturating_add(limit).min(self.pools.len() as u64)).filter_map(|i| self.pools.get(i as u32).cloned()).collect()
    }

    /// The pool a receipt settled, if it settled one. Sellers serve each member once against that receipt.
    pub fn get_pool_of_receipt(&self, receipt_id: u64) -> Option<Pool> {
        self.get_pool(*self.pool_of_receipt.get(&receipt_id)?)
    }

    // ---------- internal ----------

    fn from_v2(old: AxiomPayV2) -> Self {
        Self {
            wallets: old.wallets,
            agent_owner: old.agent_owner,
            approvals: old.approvals,
            services: old.services,
            receipts: old.receipts,
            commitments: old.commitments,
            commitment_index: old.commitment_index,
            credits: LookupMap::new(Key::Credits),
            owner_agents: LookupMap::new(Key::OwnerAgents),
            jobs: Vector::new(Key::Jobs),
            bids: LookupMap::new(Key::Bids),
            seller_stats: LookupMap::new(Key::SellerStats),
            pools: Vector::new(Key::Pools),
            pool_of_receipt: LookupMap::new(Key::PoolOfReceipt),
        }
    }

    fn wallet_mut(&mut self, owner: &AccountId) -> &mut Wallet {
        self.wallets.get_mut(owner).unwrap_or_else(|| env::panic_str("no wallet for this account; call open_wallet"))
    }

    fn owner_of(&self, agent: &AccountId) -> AccountId {
        self.agent_owner.get(agent).unwrap_or_else(|| env::panic_str("caller is not an agent of any wallet")).clone()
    }

    fn job(&self, id: u64) -> &Job {
        self.jobs.get(id as u32).unwrap_or_else(|| env::panic_str("no such job"))
    }

    fn pool(&self, id: u64) -> &Pool {
        self.pools.get(id as u32).unwrap_or_else(|| env::panic_str("no such pool"))
    }

    fn require_approved(&self, owner: &AccountId, service_id: &str, price: u128) {
        match self.approvals.get(&approval_key(owner, service_id)) {
            None => env::panic_str("service is not approved by the owner"),
            Some(p) if *p != price => env::panic_str("price changed since the owner approved it"),
            Some(_) => {}
        }
    }

    /// Debits `amount` from `owner`'s wallet under its per-call cap, daily cap and budget.
    fn debit(&mut self, owner: &AccountId, amount: u128) {
        let w = self.wallets.get_mut(owner).unwrap();
        require!(amount <= w.max_per_call.0, "price exceeds max_per_call");
        if w.spent_day != today() {
            w.spent_day = today();
            w.spent_today = U128(0);
        }
        require!(w.spent_today.0 + amount <= w.daily_cap.0, "daily cap reached");
        require!(amount <= w.budget.0, "budget exhausted");
        w.budget = U128(w.budget.0 - amount);
        w.spent_today = U128(w.spent_today.0 + amount);
        w.payments += 1;
    }

    /// Stores a receipt, pays the provider and emits the event. The money has already been debited.
    #[allow(clippy::too_many_arguments)]
    fn settle(
        &mut self,
        service_id: String,
        provider: AccountId,
        payer: AccountId,
        owner: AccountId,
        amount: u128,
        request_hash: String,
        memo: String,
    ) -> Receipt {
        let receipt = Receipt {
            id: self.receipts.len() as u64,
            service_id: service_id.clone(),
            provider: provider.clone(),
            payer,
            owner,
            amount: U128(amount),
            request_hash,
            memo,
            timestamp_ns: env::block_timestamp(),
            block_height: env::block_height(),
        };
        self.receipts.push(receipt.clone());
        if amount > 0 {
            Promise::new(provider).transfer(NearToken::from_yoctonear(amount)).detach();
        }
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"axiom-pay\",\"version\":\"3\",\"event\":\"paid\",\"data\":{{\"receipt\":{},\"service\":\"{}\",\"amount\":\"{}\"}}}}",
            receipt.id, service_id, amount
        ));
        receipt
    }

    fn join(&mut self, pool_id: u64, before: u64) -> Pool {
        let agent = env::predecessor_account_id();
        let pool = self.pool(pool_id).clone();
        require!(pool.status == PoolStatus::Open, "pool is not open");
        require!(env::block_timestamp() < pool.deadline_ns, "pool has expired");
        require!(!pool.members.contains(&agent), "already in this pool");
        let owner = self.owner_of(&agent);
        require!(!pool.owners.contains(&owner), "another agent of your wallet is already in this pool");
        self.require_approved(&owner, &pool.service_id, pool.price.0);
        // Equal shares; the last member also pays the remainder of the division.
        let n = pool.members.len() as u32;
        let mut share = pool.price.0 / pool.shares as u128;
        if n + 1 == pool.shares {
            share += pool.price.0 % pool.shares as u128;
        }
        self.debit(&owner, share);
        let p = self.pools.get_mut(pool_id as u32).unwrap();
        p.members.push(agent);
        p.owners.push(owner);
        p.paid.push(U128(share));
        let filled = p.members.len() as u32 == p.shares;
        self.pools.flush();
        self.charge_storage_refund(before);
        if filled {
            let p = self.pool(pool_id).clone();
            let memo = format!("pool #{pool_id} ({} buyers) {}", p.shares, p.memo).chars().take(MAX_TEXT_LEN).collect();
            let r = self.settle(
                p.service_id.clone(),
                p.provider.clone(),
                p.members[0].clone(),
                p.owners[0].clone(),
                p.price.0,
                p.request_hash.clone(),
                memo,
            );
            if let Some(s) = self.services.get_mut(&p.service_id) {
                s.calls += 1;
                s.earned = U128(s.earned.0 + p.price.0);
            }
            self.pool_of_receipt.insert(r.id, pool_id);
            let p = self.pools.get_mut(pool_id as u32).unwrap();
            p.status = PoolStatus::Filled;
            p.receipt = Some(r.id);
        }
        self.pool(pool_id).clone()
    }

    /// Charges the storage added since `before` to the attached deposit, then to the caller's storage credit.
    /// Returns the unused part of the deposit.
    fn charge_storage(&mut self, before: u64, deposit: u128) -> u128 {
        let used = env::storage_usage().saturating_sub(before) as u128;
        let cost = used * env::storage_byte_cost().as_yoctonear();
        if deposit >= cost {
            return deposit - cost;
        }
        let payer = env::predecessor_account_id();
        let have = self.credits.get(&payer).copied().unwrap_or(0);
        let short = cost - deposit;
        require!(
            have >= short,
            format!("attach at least {cost} yoctoNEAR for storage, or prepay storage credit with fund_storage")
        );
        self.credits.insert(payer, have - short);
        0
    }

    /// `charge_storage`, refunding any unused deposit to the caller.
    fn charge_storage_refund(&mut self, before: u64) {
        let refund = self.charge_storage(before, env::attached_deposit().as_yoctonear());
        if refund > 0 {
            Promise::new(env::predecessor_account_id()).transfer(NearToken::from_yoctonear(refund)).detach();
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    const NEAR: u128 = 10u128.pow(24);
    const H: &str = "a3f1c2d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9a";
    const SVC: &str = "axiom-signals";

    fn acc(s: &str) -> AccountId {
        s.parse().unwrap()
    }

    fn ctx(caller: &str, deposit: u128, day: u64) {
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(acc(caller))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .block_timestamp(day * NANOS_PER_DAY + 1)
            .build());
    }

    /// Seller registers a service at 0.01 NEAR; buyer opens a wallet (cap 0.1/call, 0.25/day) and approves it.
    fn setup() -> AxiomPay {
        ctx("deployer.testnet", 0, 1);
        let mut c = AxiomPay::new();
        ctx("seller.testnet", NEAR / 10, 1);
        c.register_service(SVC.into(), U128(NEAR / 100), "Axiom daily signals".into());
        ctx("buyer.testnet", NEAR + NEAR / 10, 1);
        let w = c.open_wallet(acc("bot.testnet"), U128(NEAR / 10), U128(NEAR / 4));
        assert!(w.budget.0 > NEAR && w.budget.0 < NEAR + NEAR / 10, "storage taken from the deposit");
        ctx("buyer.testnet", 0, 1);
        c.approve_service(SVC.into(), U128(NEAR / 100));
        c
    }

    fn pay(c: &mut AxiomPay) -> Receipt {
        c.pay(SVC.into(), H.into(), "2026-11-09".into())
    }

    #[test]
    fn agent_pays_approved_service_from_owners_wallet() {
        let mut c = setup();
        let budget = c.get_wallet(acc("buyer.testnet")).unwrap().budget.0;
        ctx("bot.testnet", 0, 1);
        let r = pay(&mut c);
        assert_eq!((r.id, r.provider.as_str(), r.owner.as_str(), r.amount.0), (0, "seller.testnet", "buyer.testnet", NEAR / 100));
        let w = c.get_wallet_of_agent(acc("bot.testnet")).unwrap();
        assert_eq!(w.budget.0, budget - NEAR / 100);
        assert_eq!(w.spent_today.0, NEAR / 100);
        assert_eq!(c.get_service(SVC.into()).unwrap().calls, 1);
        assert_eq!(c.get_receipt(0).unwrap().request_hash, H);
    }

    #[test]
    #[should_panic(expected = "caller is not an agent of any wallet")]
    fn stranger_cannot_pay() {
        let mut c = setup();
        ctx("mallory.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    #[should_panic(expected = "service is not approved")]
    fn unapproved_service_cannot_be_paid() {
        let mut c = setup();
        ctx("buyer.testnet", 0, 1);
        c.revoke_service(SVC.into());
        ctx("bot.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    #[should_panic(expected = "price changed since the owner approved it")]
    fn price_change_blocks_payment_until_reapproved() {
        let mut c = setup();
        ctx("seller.testnet", NEAR / 10, 1);
        c.register_service(SVC.into(), U128(NEAR / 20), "now pricier".into());
        ctx("bot.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    fn reapproval_after_price_change_works() {
        let mut c = setup();
        ctx("seller.testnet", NEAR / 10, 1);
        c.register_service(SVC.into(), U128(NEAR / 20), "now pricier".into());
        ctx("buyer.testnet", 0, 1);
        c.approve_service(SVC.into(), U128(NEAR / 20));
        ctx("bot.testnet", 0, 1);
        assert_eq!(pay(&mut c).amount.0, NEAR / 20);
    }

    #[test]
    #[should_panic(expected = "daily cap reached")]
    fn enforces_daily_cap() {
        let mut c = setup();
        ctx("bot.testnet", 0, 1);
        for _ in 0..26 {
            pay(&mut c);
        }
    }

    #[test]
    fn daily_cap_resets_next_day() {
        let mut c = setup();
        ctx("bot.testnet", 0, 1);
        for _ in 0..25 {
            pay(&mut c);
        }
        ctx("bot.testnet", 0, 2);
        pay(&mut c);
        assert_eq!(c.get_wallet(acc("buyer.testnet")).unwrap().spent_today.0, NEAR / 100);
    }

    #[test]
    #[should_panic(expected = "price exceeds max_per_call")]
    fn enforces_max_per_call() {
        let mut c = setup();
        ctx("buyer.testnet", 0, 1);
        c.set_policy(U128(NEAR / 1000), U128(NEAR));
        ctx("bot.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    #[should_panic(expected = "budget exhausted")]
    fn enforces_budget() {
        let mut c = setup();
        ctx("buyer.testnet", 0, 1);
        let b = c.get_wallet(acc("buyer.testnet")).unwrap().budget.0;
        let _ = c.withdraw(U128(b - NEAR / 200));
        ctx("bot.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    #[should_panic(expected = "service_id belongs to another provider")]
    fn cannot_hijack_service() {
        let mut c = setup();
        ctx("mallory.testnet", NEAR / 10, 1);
        c.register_service(SVC.into(), U128(NEAR / 100), "mine now".into());
    }

    #[test]
    #[should_panic(expected = "agent already spends from another wallet")]
    fn agent_cannot_serve_two_wallets() {
        let mut c = setup();
        ctx("other.testnet", NEAR / 10, 1);
        c.open_wallet(acc("bot.testnet"), U128(1), U128(1));
    }

    #[test]
    fn set_agent_moves_spending_rights() {
        let mut c = setup();
        ctx("buyer.testnet", 0, 1);
        c.set_agent(acc("bot2.testnet"));
        assert!(c.get_wallet_of_agent(acc("bot.testnet")).is_none());
        ctx("bot2.testnet", 0, 1);
        pay(&mut c);
    }

    #[test]
    fn commitments_are_per_committer() {
        let mut c = setup();
        ctx("seller.testnet", NEAR / 100, 3);
        let k = c.commit(H.into(), "NVDA breakout-52w 2026-11-06".into());
        assert_eq!((k.id, k.committer.as_str()), (0, "seller.testnet"));
        let found = c.find_commitment(acc("seller.testnet"), H.into()).unwrap();
        assert_eq!(found.timestamp_ns, 3 * NANOS_PER_DAY + 1);
        assert!(c.find_commitment(acc("other.testnet"), H.into()).is_none());
        // Someone else committing the same hash does not take over the seller's record.
        ctx("other.testnet", NEAR / 100, 4);
        c.commit(H.into(), String::new());
        assert_eq!(c.find_commitment(acc("seller.testnet"), H.into()).unwrap().timestamp_ns, 3 * NANOS_PER_DAY + 1);
        assert_eq!(c.commitment_count(), 2);
    }

    #[test]
    #[should_panic(expected = "hash already committed")]
    fn cannot_recommit() {
        let mut c = setup();
        ctx("seller.testnet", NEAR / 100, 3);
        c.commit(H.into(), String::new());
        c.commit(H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "for storage")]
    fn commit_requires_storage_deposit() {
        let mut c = setup();
        ctx("seller.testnet", 0, 3);
        c.commit(H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "hash must be 64 lowercase hex chars")]
    fn rejects_bad_hash() {
        let mut c = setup();
        ctx("seller.testnet", NEAR / 100, 1);
        c.commit("not-a-hash".into(), String::new());
    }

    // ---------- v3: fleets, storage credit, jobs, pools ----------

    #[test]
    fn added_agents_share_the_wallet_and_can_be_removed() {
        let mut c = setup();
        ctx("buyer.testnet", NEAR / 100, 1);
        c.add_agent(acc("bot2.testnet"));
        assert_eq!(c.get_agents(acc("buyer.testnet")).len(), 2);
        ctx("bot2.testnet", 0, 1);
        pay(&mut c);
        assert_eq!(c.get_wallet(acc("buyer.testnet")).unwrap().spent_today.0, NEAR / 100);
        ctx("buyer.testnet", 0, 1);
        c.remove_agent(acc("bot2.testnet"));
        assert!(c.get_wallet_of_agent(acc("bot2.testnet")).is_none());
    }

    #[test]
    fn storage_credit_pays_when_no_deposit_is_attached() {
        let mut c = setup();
        ctx("buyer.testnet", NEAR / 10, 1);
        c.fund_storage(Some(acc("seller.testnet")));
        ctx("seller.testnet", 0, 3);
        c.commit(H.into(), "no deposit".into());
        assert!(c.get_storage_credit(acc("seller.testnet")).0 < NEAR / 10);
    }

    /// Bot posts a job; two sellers bid; bot awards the cheaper one.
    fn job_with_bids(c: &mut AxiomPay) -> u64 {
        ctx("buyer.testnet", NEAR / 10, 1);
        c.fund_storage(Some(acc("bot.testnet")));
        ctx("bot.testnet", 0, 1);
        let job = c.post_job("sec-red-flags|JPM|2026-10-10".into(), U128(NEAR / 20), 30);
        ctx("seller.testnet", NEAR / 100, 1);
        c.bid(job.id, U128(NEAR / 50), "http://a".into());
        ctx("rival.testnet", NEAR / 100, 1);
        c.bid(job.id, U128(NEAR / 40), "http://b".into());
        c.bid(job.id, U128(NEAR / 80), "http://b".into()); // lowers its bid
        assert_eq!(c.get_bids(job.id).len(), 2);
        assert_eq!(c.get_job(job.id).unwrap().bids, 2);
        job.id
    }

    #[test]
    fn job_award_pays_winner_and_confirm_builds_reputation() {
        let mut c = setup();
        let id = job_with_bids(&mut c);
        let budget = c.get_wallet(acc("buyer.testnet")).unwrap().budget.0;
        ctx("bot.testnet", 0, 1);
        let r = c.award(id, acc("rival.testnet"));
        assert_eq!((r.service_id.as_str(), r.provider.as_str(), r.amount.0), ("job:0", "rival.testnet", NEAR / 80));
        assert_eq!(r.request_hash, job_request_hash(id, "sec-red-flags|JPM|2026-10-10"));
        assert_eq!(c.get_wallet(acc("buyer.testnet")).unwrap().budget.0, budget - NEAR / 80);
        c.confirm(id, true);
        let st = c.get_seller_stats(acc("rival.testnet"));
        assert_eq!((st.awarded, st.delivered, st.disputed), (1, 1, 0));
        assert_eq!(c.get_job(id).unwrap().status, JobStatus::Delivered);
    }

    #[test]
    #[should_panic(expected = "only the agent that posted the job can award it")]
    fn only_buyer_awards() {
        let mut c = setup();
        let id = job_with_bids(&mut c);
        ctx("rival.testnet", 0, 1);
        c.award(id, acc("rival.testnet"));
    }

    #[test]
    #[should_panic(expected = "price must be above 0 and at most the job's max_price")]
    fn bid_above_max_rejected() {
        let mut c = setup();
        let id = job_with_bids(&mut c);
        ctx("other.testnet", NEAR / 100, 1);
        c.bid(id, U128(NEAR), "http://c".into());
    }

    #[test]
    #[should_panic(expected = "job has expired")]
    fn bid_after_deadline_rejected() {
        let mut c = setup();
        let id = job_with_bids(&mut c);
        ctx("other.testnet", NEAR / 100, 2);
        c.bid(id, U128(NEAR / 100), "http://c".into());
    }

    #[test]
    #[should_panic(expected = "max_price exceeds max_per_call")]
    fn job_must_fit_policy() {
        let mut c = setup();
        ctx("bot.testnet", NEAR / 100, 1);
        c.post_job("x".into(), U128(NEAR), 30);
    }

    /// Three owners, each with an approved agent: a1/a2/a3.
    fn three_buyers() -> AxiomPay {
        let mut c = setup();
        for i in 1..=3 {
            ctx(&format!("o{i}.testnet"), NEAR, 1);
            c.open_wallet(acc(&format!("a{i}.testnet")), U128(NEAR / 10), U128(NEAR));
            ctx(&format!("o{i}.testnet"), NEAR / 10, 1);
            c.fund_storage(Some(acc(&format!("a{i}.testnet"))));
            ctx(&format!("o{i}.testnet"), 0, 1);
            c.approve_service(SVC.into(), U128(NEAR / 100));
        }
        c
    }

    #[test]
    fn pool_settles_when_last_share_arrives() {
        let mut c = three_buyers();
        ctx("a1.testnet", 0, 1);
        let p = c.open_pool(SVC.into(), H.into(), "2026-11-09".into(), 3, 30);
        assert_eq!((p.status.clone(), p.members.len()), (PoolStatus::Open, 1));
        ctx("a2.testnet", 0, 1);
        c.join_pool(p.id);
        assert_eq!(c.receipt_count(), 0, "seller not paid before the pool fills");
        ctx("a3.testnet", 0, 1);
        let p = c.join_pool(p.id);
        assert_eq!(p.status, PoolStatus::Filled);
        let paid: u128 = p.paid.iter().map(|x| x.0).sum();
        assert_eq!(paid, NEAR / 100, "shares add up to the price exactly");
        let r = c.get_receipt(p.receipt.unwrap()).unwrap();
        assert_eq!((r.amount.0, r.provider.as_str(), r.request_hash.as_str()), (NEAR / 100, "seller.testnet", H));
        assert_eq!(c.get_pool_of_receipt(r.id).unwrap().members.len(), 3);
        assert_eq!(c.get_service(SVC.into()).unwrap().calls, 1);
    }

    #[test]
    fn unfilled_pool_refunds_after_deadline() {
        let mut c = three_buyers();
        let b = c.get_wallet(acc("o1.testnet")).unwrap().budget.0;
        ctx("a1.testnet", 0, 1);
        let p = c.open_pool(SVC.into(), H.into(), String::new(), 3, 30);
        assert!(c.get_wallet(acc("o1.testnet")).unwrap().budget.0 < b);
        ctx("anyone.testnet", 0, 2);
        assert_eq!(c.refund_pool(p.id).status, PoolStatus::Refunded);
        assert_eq!(c.get_wallet(acc("o1.testnet")).unwrap().budget.0, b);
    }

    #[test]
    #[should_panic(expected = "service is not approved by the owner")]
    fn pool_member_needs_approval() {
        let mut c = three_buyers();
        ctx("a1.testnet", 0, 1);
        let p = c.open_pool(SVC.into(), H.into(), String::new(), 2, 30);
        ctx("o2.testnet", 0, 1);
        c.revoke_service(SVC.into());
        ctx("a2.testnet", 0, 1);
        c.join_pool(p.id);
    }

    #[test]
    #[should_panic(expected = "already in this pool")]
    fn cannot_join_twice() {
        let mut c = three_buyers();
        ctx("a1.testnet", 0, 1);
        let p = c.open_pool(SVC.into(), H.into(), String::new(), 3, 30);
        c.join_pool(p.id);
    }
}
