//! Axiom Pay: a marketplace where AI agents sell research to other agents, inside budgets humans control.
//!
//! * **Wallets.** Any human (owner) opens a wallet for one agent account: a budget, a per-call cap, a daily
//!   cap, and an allowlist of services at approved prices. The agent can spend only inside those limits.
//! * **Services.** Any account can sell a service at a price per call. Payment goes straight to the seller.
//!   If the seller changes the price, existing approvals stop matching and buyers cannot pay until their
//!   owner approves the new price.
//! * **Receipts.** Each payment stores a receipt bound to a hash of the exact request. The seller checks the
//!   receipt (a free view call) before serving.
//! * **Commitments.** Any account can commit the hash of a prediction (for Axiom: a trade signal). The block
//!   time proves it existed before its outcome. Buyers use a seller's commitments to verify its track record
//!   before paying for new signals.
use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, LookupMap, Vector};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

const NANOS_PER_DAY: u64 = 86_400 * 1_000_000_000;
const MAX_ID_LEN: usize = 64;
const MAX_TEXT_LEN: usize = 280;

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
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Wallet {
    pub owner: AccountId,
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
    pub service_id: String,
    pub provider: AccountId,
    /// Agent that paid, and the owner whose wallet funded it.
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

#[near]
impl AxiomPay {
    #[init]
    pub fn new() -> Self {
        Self {
            wallets: LookupMap::new(Key::Wallets),
            agent_owner: LookupMap::new(Key::AgentOwner),
            approvals: LookupMap::new(Key::Approvals),
            services: IterableMap::new(Key::Services),
            receipts: Vector::new(Key::Receipts),
            commitments: Vector::new(Key::Commitments),
            commitment_index: LookupMap::new(Key::CommitmentIndex),
        }
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
        let rest = self.storage_from_deposit(before, env::attached_deposit().as_yoctonear());
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

    pub fn set_agent(&mut self, agent: AccountId) {
        let owner = env::predecessor_account_id();
        require!(!self.agent_owner.contains_key(&agent), "agent already spends from another wallet");
        let old = self.wallet_mut(&owner).agent.clone();
        self.agent_owner.remove(&old);
        self.agent_owner.insert(agent.clone(), owner.clone());
        self.wallet_mut(&owner).agent = agent;
    }

    /// Allowlists a service for the caller's agent at `price`, which must be the current price.
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

    // ---------- sellers ----------

    /// Registers or reprices a service owned by the caller. The caller pays the storage it uses.
    #[payable]
    pub fn register_service(&mut self, service_id: String, price: U128, description: String) {
        require!(
            !service_id.is_empty() && service_id.len() <= MAX_ID_LEN && !service_id.contains('|'),
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
        let refund = self.storage_from_deposit(before, env::attached_deposit().as_yoctonear());
        if refund > 0 {
            Promise::new(env::predecessor_account_id()).transfer(NearToken::from_yoctonear(refund)).detach();
        }
    }

    // ---------- agents ----------

    /// Pays one call of `service_id` from the caller's wallet. Enforces the owner's approval at the current
    /// price, the per-call cap, the daily cap and the budget, then pays the seller.
    pub fn pay(&mut self, service_id: String, request_hash: String, memo: String) -> Receipt {
        let agent = env::predecessor_account_id();
        require_hash(&request_hash);
        require_text(&memo, MAX_TEXT_LEN, "memo");
        let owner = self.agent_owner.get(&agent).unwrap_or_else(|| env::panic_str("caller is not an agent of any wallet")).clone();
        let svc = self.services.get(&service_id).unwrap_or_else(|| env::panic_str("unknown service")).clone();
        let price = svc.price.0;
        match self.approvals.get(&approval_key(&owner, &service_id)) {
            None => env::panic_str("service is not approved by the owner"),
            Some(p) if *p != price => env::panic_str("price changed since the owner approved it"),
            Some(_) => {}
        }
        let w = self.wallets.get_mut(&owner).unwrap();
        require!(price <= w.max_per_call.0, "price exceeds max_per_call");
        if w.spent_day != today() {
            w.spent_day = today();
            w.spent_today = U128(0);
        }
        require!(w.spent_today.0 + price <= w.daily_cap.0, "daily cap reached");
        require!(price <= w.budget.0, "budget exhausted");
        w.budget = U128(w.budget.0 - price);
        w.spent_today = U128(w.spent_today.0 + price);
        w.payments += 1;

        let receipt = Receipt {
            id: self.receipts.len() as u64,
            service_id: service_id.clone(),
            provider: svc.provider.clone(),
            payer: agent,
            owner,
            amount: U128(price),
            request_hash,
            memo,
            timestamp_ns: env::block_timestamp(),
            block_height: env::block_height(),
        };
        self.receipts.push(receipt.clone());
        let s = self.services.get_mut(&service_id).unwrap();
        s.calls += 1;
        s.earned = U128(s.earned.0 + price);
        if price > 0 {
            Promise::new(svc.provider).transfer(NearToken::from_yoctonear(price)).detach();
        }
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"axiom-pay\",\"version\":\"2\",\"event\":\"paid\",\"data\":{{\"receipt\":{},\"service\":\"{}\",\"amount\":\"{}\"}}}}",
            receipt.id, service_id, price
        ));
        receipt
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
        let refund = self.storage_from_deposit(before, env::attached_deposit().as_yoctonear());
        if refund > 0 {
            Promise::new(env::predecessor_account_id()).transfer(NearToken::from_yoctonear(refund)).detach();
        }
        c
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

    /// The price `owner` approved for `service_id`, if any.
    pub fn get_approval(&self, owner: AccountId, service_id: String) -> Option<U128> {
        self.approvals.get(&approval_key(&owner, &service_id)).map(|p| U128(*p))
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

    // ---------- internal ----------

    fn wallet_mut(&mut self, owner: &AccountId) -> &mut Wallet {
        self.wallets.get_mut(owner).unwrap_or_else(|| env::panic_str("no wallet for this account; call open_wallet"))
    }

    /// Requires `deposit` to cover the storage added since `before`; returns what is left over.
    fn storage_from_deposit(&self, before: u64, deposit: u128) -> u128 {
        let used = env::storage_usage().saturating_sub(before) as u128;
        let cost = used * env::storage_byte_cost().as_yoctonear();
        require!(deposit >= cost, format!("attach at least {cost} yoctoNEAR for storage"));
        deposit - cost
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
}
