//! Axiom Pay: the economic layer of the Axiom research agent.
//!
//! * The **owner** (a human) funds a budget and sets the spending policy for one **agent** account.
//! * **Providers** (other agents or data vendors) register priced services. A service can be paid only
//!   after the owner approves it, and approval resets whenever the provider changes the price.
//! * The **agent** pays for a call with `pay`. The contract enforces the allowlist, the per-call cap and the
//!   daily cap, sends the price to the provider, and stores a receipt bound to a hash of the request.
//!   The provider checks that receipt (a free view call) before it serves the request.
//! * The agent **commits** a hash of every research signal *before* its outcome is known. Anyone can later
//!   recompute the hash and check when the signal was committed, so the track record cannot be rewritten.
use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, LookupMap, Vector};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

const NANOS_PER_DAY: u64 = 86_400 * 1_000_000_000;
const MAX_ID_LEN: usize = 64;
const MAX_TEXT_LEN: usize = 280;

#[near]
#[derive(BorshStorageKey)]
enum Key {
    Services,
    Receipts,
    Commitments,
    CommitmentByHash,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Service {
    pub provider: AccountId,
    /// Price per call in yoctoNEAR.
    pub price: U128,
    pub description: String,
    /// Only approved services can be paid. Changing the price clears approval.
    pub approved: bool,
    pub calls: u64,
    pub earned: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Receipt {
    pub id: u64,
    pub service_id: String,
    pub provider: AccountId,
    pub payer: AccountId,
    pub amount: U128,
    /// Hex SHA-256 of the request the agent is paying for. The provider serves only a matching request.
    pub request_hash: String,
    pub memo: String,
    pub timestamp_ns: u64,
    pub block_height: u64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Commitment {
    pub id: u64,
    /// Hex SHA-256 of the canonical signal record.
    pub hash: String,
    pub label: String,
    pub timestamp_ns: u64,
    pub block_height: u64,
}

#[near(serializers = [json])]
pub struct Policy {
    pub owner: AccountId,
    pub agent: AccountId,
    pub max_per_call: U128,
    pub daily_cap: U128,
    pub budget: U128,
    pub spent_today: U128,
    pub receipts: u64,
    pub commitments: u64,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct AxiomPay {
    owner: AccountId,
    agent: AccountId,
    max_per_call: u128,
    daily_cap: u128,
    /// Deposited funds not yet spent or withdrawn. Kept apart from the balance that pays for storage.
    budget: u128,
    spent_day: u64,
    spent_today: u128,
    services: IterableMap<String, Service>,
    receipts: Vector<Receipt>,
    commitments: Vector<Commitment>,
    commitment_by_hash: LookupMap<String, u64>,
}

fn require_hash(h: &str) {
    require!(h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "hash must be 64 lowercase hex chars");
}

fn require_text(s: &str, max: usize, what: &str) {
    require!(s.len() <= max, format!("{what} is longer than {max} bytes"));
}

#[near]
impl AxiomPay {
    #[init]
    pub fn new(owner: AccountId, agent: AccountId, max_per_call: U128, daily_cap: U128) -> Self {
        Self {
            owner,
            agent,
            max_per_call: max_per_call.0,
            daily_cap: daily_cap.0,
            budget: 0,
            spent_day: 0,
            spent_today: 0,
            services: IterableMap::new(Key::Services),
            receipts: Vector::new(Key::Receipts),
            commitments: Vector::new(Key::Commitments),
            commitment_by_hash: LookupMap::new(Key::CommitmentByHash),
        }
    }

    // ---------- owner ----------

    /// Adds the attached deposit to the agent's budget.
    #[payable]
    pub fn deposit(&mut self) -> U128 {
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount > 0, "attach a deposit");
        self.budget += amount;
        env::log_str(&format!("budget +{amount} yocto by {}", env::predecessor_account_id()));
        U128(self.budget)
    }

    pub fn withdraw(&mut self, amount: U128) -> Promise {
        self.assert_owner();
        require!(amount.0 <= self.budget, "amount exceeds budget");
        self.budget -= amount.0;
        Promise::new(self.owner.clone()).transfer(NearToken::from_yoctonear(amount.0))
    }

    pub fn set_policy(&mut self, max_per_call: U128, daily_cap: U128) {
        self.assert_owner();
        self.max_per_call = max_per_call.0;
        self.daily_cap = daily_cap.0;
    }

    pub fn set_agent(&mut self, agent: AccountId) {
        self.assert_owner();
        self.agent = agent;
    }

    /// Allowlists a service at its current price. `price` must match, so the owner approves what they saw.
    pub fn approve_service(&mut self, service_id: String, price: U128) {
        self.assert_owner();
        let svc = self.services.get_mut(&service_id).unwrap_or_else(|| env::panic_str("unknown service"));
        require!(svc.price == price, "price changed since you looked; re-check the service");
        svc.approved = true;
    }

    pub fn revoke_service(&mut self, service_id: String) {
        self.assert_owner();
        let svc = self.services.get_mut(&service_id).unwrap_or_else(|| env::panic_str("unknown service"));
        svc.approved = false;
    }

    // ---------- providers ----------

    /// Registers or updates a service owned by the caller. The caller pays the storage it uses.
    #[payable]
    pub fn register_service(&mut self, service_id: String, price: U128, description: String) {
        require!(!service_id.is_empty() && service_id.len() <= MAX_ID_LEN, "bad service_id");
        require_text(&description, MAX_TEXT_LEN, "description");
        let provider = env::predecessor_account_id();
        let before = env::storage_usage();
        let svc = match self.services.get(&service_id) {
            Some(old) => {
                require!(old.provider == provider, "service_id belongs to another provider");
                Service {
                    approved: old.approved && old.price == price,
                    price,
                    description,
                    ..old.clone()
                }
            }
            None => Service { provider, price, description, approved: false, calls: 0, earned: U128(0) },
        };
        self.services.insert(service_id, svc);
        self.services.flush();
        self.charge_storage(before);
    }

    // ---------- agent ----------

    /// Pays one call of `service_id`. Enforces approval, per-call cap, daily cap and budget.
    pub fn pay(&mut self, service_id: String, request_hash: String, memo: String) -> Receipt {
        self.assert_agent();
        require_hash(&request_hash);
        require_text(&memo, MAX_TEXT_LEN, "memo");
        let svc = self.services.get(&service_id).unwrap_or_else(|| env::panic_str("unknown service")).clone();
        require!(svc.approved, "service is not approved by the owner");
        let price = svc.price.0;
        require!(price <= self.max_per_call, "price exceeds max_per_call");
        self.roll_day();
        require!(self.spent_today + price <= self.daily_cap, "daily cap reached");
        require!(price <= self.budget, "budget exhausted");

        self.budget -= price;
        self.spent_today += price;
        let receipt = Receipt {
            id: self.receipts.len() as u64,
            service_id: service_id.clone(),
            provider: svc.provider.clone(),
            payer: env::predecessor_account_id(),
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
        env::log_str(&format!("EVENT_JSON:{{\"standard\":\"axiom-pay\",\"event\":\"paid\",\"data\":{{\"receipt\":{},\"service\":\"{}\",\"amount\":\"{}\"}}}}", receipt.id, service_id, price));
        receipt
    }

    /// Records a signal hash. Each hash can be committed once; the first commitment's time is what counts.
    pub fn commit(&mut self, hash: String, label: String) -> Commitment {
        self.assert_agent();
        require_hash(&hash);
        require_text(&label, MAX_TEXT_LEN, "label");
        require!(!self.commitment_by_hash.contains_key(&hash), "hash already committed");
        let c = Commitment {
            id: self.commitments.len() as u64,
            hash: hash.clone(),
            label,
            timestamp_ns: env::block_timestamp(),
            block_height: env::block_height(),
        };
        self.commitments.push(c.clone());
        self.commitment_by_hash.insert(hash, c.id);
        c
    }

    // ---------- views ----------

    pub fn get_policy(&self) -> Policy {
        let today = env::block_timestamp() / NANOS_PER_DAY;
        Policy {
            owner: self.owner.clone(),
            agent: self.agent.clone(),
            max_per_call: U128(self.max_per_call),
            daily_cap: U128(self.daily_cap),
            budget: U128(self.budget),
            spent_today: U128(if self.spent_day == today { self.spent_today } else { 0 }),
            receipts: self.receipts.len() as u64,
            commitments: self.commitments.len() as u64,
        }
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

    /// Receipts `[from, from+limit)`.
    pub fn get_receipts(&self, from: u64, limit: u64) -> Vec<Receipt> {
        (from..from.saturating_add(limit).min(self.receipts.len() as u64))
            .filter_map(|i| self.receipts.get(i as u32).cloned())
            .collect()
    }

    pub fn get_commitments(&self, from: u64, limit: u64) -> Vec<Commitment> {
        (from..from.saturating_add(limit).min(self.commitments.len() as u64))
            .filter_map(|i| self.commitments.get(i as u32).cloned())
            .collect()
    }

    pub fn find_commitment(&self, hash: String) -> Option<Commitment> {
        let id = *self.commitment_by_hash.get(&hash)?;
        self.commitments.get(id as u32).cloned()
    }

    // ---------- internal ----------

    fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner, "only the owner");
    }

    fn assert_agent(&self) {
        require!(env::predecessor_account_id() == self.agent, "only the agent");
    }

    fn roll_day(&mut self) {
        let today = env::block_timestamp() / NANOS_PER_DAY;
        if today != self.spent_day {
            self.spent_day = today;
            self.spent_today = 0;
        }
    }

    /// Requires the attached deposit to cover storage added since `before` and refunds the rest.
    fn charge_storage(&mut self, before: u64) {
        let used = env::storage_usage().saturating_sub(before) as u128;
        let cost = used * env::storage_byte_cost().as_yoctonear();
        let attached = env::attached_deposit().as_yoctonear();
        require!(attached >= cost, format!("attach at least {cost} yoctoNEAR for storage"));
        let refund = attached - cost;
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

    fn setup() -> AxiomPay {
        ctx("owner.testnet", 0, 1);
        let mut c = AxiomPay::new(acc("owner.testnet"), acc("agent.testnet"), U128(NEAR / 10), U128(NEAR / 4));
        ctx("owner.testnet", NEAR, 1);
        c.deposit();
        ctx("sec.testnet", NEAR / 10, 1);
        c.register_service("sec-red-flags".into(), U128(NEAR / 100), "SEC filing red flags".into());
        ctx("owner.testnet", 0, 1);
        c.approve_service("sec-red-flags".into(), U128(NEAR / 100));
        c
    }

    #[test]
    fn pays_approved_service_and_records_receipt() {
        let mut c = setup();
        ctx("agent.testnet", 0, 1);
        let r = c.pay("sec-red-flags".into(), H.into(), "NVDA 2026-11-06".into());
        assert_eq!(r.id, 0);
        assert_eq!(r.provider, acc("sec.testnet"));
        assert_eq!(r.amount.0, NEAR / 100);
        let p = c.get_policy();
        assert_eq!(p.budget.0, NEAR - NEAR / 100);
        assert_eq!(p.spent_today.0, NEAR / 100);
        assert_eq!(c.get_service("sec-red-flags".into()).unwrap().calls, 1);
        assert_eq!(c.get_receipt(0).unwrap().request_hash, H);
    }

    #[test]
    #[should_panic(expected = "only the agent")]
    fn only_agent_can_pay() {
        let mut c = setup();
        ctx("mallory.testnet", 0, 1);
        c.pay("sec-red-flags".into(), H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "daily cap reached")]
    fn enforces_daily_cap() {
        let mut c = setup();
        ctx("agent.testnet", 0, 1);
        for _ in 0..26 {
            c.pay("sec-red-flags".into(), H.into(), String::new());
        }
    }

    #[test]
    fn daily_cap_resets_next_day() {
        let mut c = setup();
        ctx("agent.testnet", 0, 1);
        for _ in 0..25 {
            c.pay("sec-red-flags".into(), H.into(), String::new());
        }
        ctx("agent.testnet", 0, 2);
        c.pay("sec-red-flags".into(), H.into(), String::new());
        assert_eq!(c.get_policy().spent_today.0, NEAR / 100);
    }

    #[test]
    #[should_panic(expected = "service is not approved")]
    fn price_change_clears_approval() {
        let mut c = setup();
        ctx("sec.testnet", NEAR / 10, 1);
        c.register_service("sec-red-flags".into(), U128(NEAR / 20), "now pricier".into());
        ctx("agent.testnet", 0, 1);
        c.pay("sec-red-flags".into(), H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "service_id belongs to another provider")]
    fn cannot_hijack_service() {
        let mut c = setup();
        ctx("mallory.testnet", NEAR / 10, 1);
        c.register_service("sec-red-flags".into(), U128(NEAR / 100), "mine now".into());
    }

    #[test]
    #[should_panic(expected = "price exceeds max_per_call")]
    fn enforces_max_per_call() {
        let mut c = setup();
        ctx("owner.testnet", 0, 1);
        c.set_policy(U128(NEAR / 1000), U128(NEAR));
        ctx("agent.testnet", 0, 1);
        c.pay("sec-red-flags".into(), H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "budget exhausted")]
    fn enforces_budget() {
        let mut c = setup();
        ctx("owner.testnet", 0, 1);
        let _ = c.withdraw(U128(NEAR - NEAR / 200));
        ctx("agent.testnet", 0, 1);
        c.pay("sec-red-flags".into(), H.into(), String::new());
    }

    #[test]
    fn commits_once_and_finds_by_hash() {
        let mut c = setup();
        ctx("agent.testnet", 0, 3);
        let k = c.commit(H.into(), "NVDA breakout-52w 2026-11-06".into());
        assert_eq!(k.id, 0);
        let found = c.find_commitment(H.into()).unwrap();
        assert_eq!(found.timestamp_ns, 3 * NANOS_PER_DAY + 1);
        assert_eq!(c.get_commitments(0, 10).len(), 1);
        assert!(c.find_commitment("0".repeat(64)).is_none());
    }

    #[test]
    #[should_panic(expected = "hash already committed")]
    fn cannot_recommit() {
        let mut c = setup();
        ctx("agent.testnet", 0, 3);
        c.commit(H.into(), String::new());
        c.commit(H.into(), String::new());
    }

    #[test]
    #[should_panic(expected = "hash must be 64 lowercase hex chars")]
    fn rejects_bad_hash() {
        let mut c = setup();
        ctx("agent.testnet", 0, 1);
        c.commit("not-a-hash".into(), String::new());
    }
}
