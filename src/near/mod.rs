//! Axiom Pay on NEAR. Axiom sells its research to other agents per call, buys evidence it needs, and commits
//! every signal on-chain before its outcome is known so buyers can verify its record before paying.
//! See `contract/` for the smart contract.
pub mod client;
pub mod provider;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::Filing;
use crate::store::StoredSignal;
use client::{fmt_near, parse_near, sha256_hex, u128_field, Near, Signer};

/// Recent SEC filings for a ticker, tagged with 8-K red-flag items.
pub const SEC_SERVICE: &str = "sec-red-flags";
/// One session's fresh Axiom signals (before the next open), with entry/stop/target.
pub const SIGNALS_SERVICE: &str = "axiom-signals";
/// The full cited research report(s) for one symbol and session.
pub const REPORT_SERVICE: &str = "axiom-report";

/// What an agent needs to act on-chain: RPC, its key, and the Axiom Pay contract.
pub struct Ctx {
    pub near: Near,
    pub signer: Signer,
    pub contract: String,
}

impl Ctx {
    pub fn from_env(account: Option<&str>) -> Result<Self> {
        let near = Near::from_env();
        let signer = Signer::load(&near.network, account)?;
        Ok(Self { near, signer, contract: contract_id()? })
    }
}

pub fn contract_id() -> Result<String> {
    std::env::var("AXIOM_PAY_CONTRACT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .context("set AXIOM_PAY_CONTRACT to the deployed Axiom Pay contract account")
}

/// The request a paid call is bound to. Buyer and seller both compute it; the receipt stores its hash.
pub fn request_line(service_id: &str, params: &[&str]) -> String {
    let mut s = format!("axiom-pay/v1|{service_id}");
    for p in params {
        s.push('|');
        s.push_str(p);
    }
    s
}

/// Canonical text of a signal. Its SHA-256 is what gets committed on-chain.
/// Fixed precision so the same signal always hashes the same, from memory or from the database.
pub fn signal_line(symbol: &str, setup: &str, as_of: NaiveDate, entry: f64, stop: f64, target: f64, max_hold: usize) -> String {
    format!("axiom-signal/v1|{symbol}|{setup}|{as_of}|{entry:.4}|{stop:.4}|{target:.4}|{max_hold}")
}

pub fn stored_hash(s: &StoredSignal) -> String {
    sha256_hex(&signal_line(&s.symbol, &s.setup, s.as_of, s.entry, s.stop, s.target, s.max_hold))
}

/// Commits a signal hash for the signer (attaching 0.01 NEAR for storage; the excess is refunded). Returns the tx hash.
pub async fn commit_signal(ctx: &Ctx, hash: &str, label: &str) -> Result<String> {
    let r = ctx.near.call(&ctx.signer, &ctx.contract, "commit", json!({"hash": hash, "label": label}), parse_near("0.01")?).await?;
    Ok(r.hash)
}

/// Earliest moment the next session's open could be known: 13:30 UTC (09:30 EDT) on the next weekday.
/// A commitment before this cannot have used the fill price.
pub fn entry_known_at(as_of: NaiveDate, fill: Option<NaiveDate>) -> DateTime<Utc> {
    let mut d = fill.unwrap_or(as_of + TimeDelta::days(1));
    while fill.is_none() && matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
        d += TimeDelta::days(1);
    }
    d.and_hms_opt(13, 30, 0).unwrap().and_utc()
}

fn ns_to_utc(ns: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp((ns / 1_000_000_000) as i64, 0).unwrap_or_default()
}

// ---------- buying ----------

/// Buys one call from a seller, x402-style: ask -> 402 with terms -> check the terms against our own request
/// hash, the contract, and our owner's on-chain approval -> pay -> redeem the receipt. Returns the JSON body.
pub async fn purchase(ctx: &Ctx, base_url: &str, service_id: &str, query: &[(&str, String)], hash_params: &[&str]) -> Result<Value> {
    let http = reqwest::Client::new();
    let url = format!("{}/v1/{service_id}", base_url.trim_end_matches('/'));

    let first = http.get(&url).query(query).send().await.with_context(|| format!("seller {url}"))?;
    if first.status().is_success() {
        return Ok(first.json().await?); // free
    }
    if first.status().as_u16() != 402 {
        bail!("seller answered HTTP {}: {}", first.status(), first.text().await.unwrap_or_default());
    }
    let terms: Value = first.json().await.context("402 without JSON terms")?;
    let request_hash = sha256_hex(&request_line(service_id, hash_params));
    if terms["request_hash"].as_str() != Some(request_hash.as_str()) {
        bail!("seller quoted a different request hash; refusing to pay");
    }
    if terms["contract"].as_str() != Some(ctx.contract.as_str()) {
        bail!("seller wants payment on {}, not our contract {}", terms["contract"], ctx.contract);
    }
    let svc = ctx.near.view(&ctx.contract, "get_service", json!({"service_id": service_id})).await?;
    if svc.is_null() {
        bail!("service {service_id} is not registered on {}", ctx.contract);
    }
    let price = u128_field(&svc, "price")?;
    let wallet = ctx.near.view(&ctx.contract, "get_wallet_of_agent", json!({"agent": ctx.signer.account_id})).await?;
    if wallet.is_null() {
        bail!("{} is not the agent of any wallet (the owner runs `axiom near open-wallet`)", ctx.signer.account_id);
    }
    let owner = wallet["owner"].as_str().unwrap_or_default();
    let approved = ctx.near.view(&ctx.contract, "get_approval", json!({"owner": owner, "service_id": service_id})).await?;
    if approved.as_str().and_then(|s| s.parse::<u128>().ok()) != Some(price) {
        bail!(
            "{owner} has not approved {service_id} at its current price {} (owner runs `axiom near approve {service_id}`)",
            fmt_near(price)
        );
    }

    let memo: String = hash_params.join(" ").chars().take(200).collect();
    let paid = ctx
        .near
        .call(&ctx.signer, &ctx.contract, "pay", json!({"service_id": service_id, "request_hash": request_hash, "memo": memo}), 0)
        .await
        .context("on-chain payment")?;
    let receipt = paid.value.as_ref().and_then(|v| v["id"].as_u64()).context("pay returned no receipt id")?;
    eprintln!(
        "    paid {} to {} for {service_id} ({memo}): receipt #{receipt}, {}",
        fmt_near(price),
        svc["provider"].as_str().unwrap_or("?"),
        ctx.near.explorer_tx(&paid.hash)
    );
    redeem(&http, &url, query, receipt).await
}

/// Redeems a paid receipt. Retries while the receipt is not yet final on the seller's RPC and while the
/// seller has a transient upstream failure (it un-redeems the receipt in that case, so retrying is safe).
async fn redeem(http: &reqwest::Client, url: &str, query: &[(&str, String)], receipt: u64) -> Result<Value> {
    let mut last = String::new();
    for attempt in 0..8u64 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500 * attempt)).await;
        }
        let resp = match http.get(url).query(query).query(&[("receipt", receipt.to_string())]).send().await {
            Ok(r) => r,
            Err(e) => {
                last = format!("{e}");
                continue;
            }
        };
        let status = resp.status();
        if status.is_success() {
            return Ok(resp.json().await?);
        }
        last = format!("HTTP {status}: {}", resp.text().await.unwrap_or_default());
        if !(last.contains("not found") || status.is_server_error()) {
            break;
        }
    }
    bail!("seller did not honour receipt #{receipt}: {last} (it can be redeemed for 15 minutes after payment)")
}

pub async fn buy_filings(ctx: &Ctx, url: &str, symbol: &str, since: NaiveDate) -> Result<Vec<Filing>> {
    let since_s = since.to_string();
    let v = purchase(ctx, url, SEC_SERVICE, &[("symbol", symbol.to_string()), ("since", since_s.clone())], &[symbol, &since_s]).await?;
    Ok(serde_json::from_value(v)?)
}

// ---------- track record ----------

/// One published signal: everything needed to recompute its hash, plus the outcome the seller reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEntry {
    pub symbol: String,
    pub setup: String,
    pub as_of: NaiveDate,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub max_hold: usize,
    pub status: String,
    pub fill_date: Option<NaiveDate>,
    pub exit_date: Option<NaiveDate>,
    /// Net return of the closed paper trade, as reported by the seller.
    pub ret: Option<f64>,
}

impl From<&StoredSignal> for RecordEntry {
    fn from(s: &StoredSignal) -> Self {
        Self {
            symbol: s.symbol.clone(),
            setup: s.setup.clone(),
            as_of: s.as_of,
            entry: s.entry,
            stop: s.stop,
            target: s.target,
            max_hold: s.max_hold,
            status: s.status.clone(),
            fill_date: s.fill_date,
            exit_date: s.exit_date,
            ret: s.ret,
        }
    }
}

impl RecordEntry {
    pub fn hash(&self) -> String {
        sha256_hex(&signal_line(&self.symbol, &self.setup, self.as_of, self.entry, self.stop, self.target, self.max_hold))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Committed before the entry price could be known.
    OnTime,
    Late,
    Missing,
}

pub struct Verified {
    pub rows: Vec<(RecordEntry, Check, Option<DateTime<Utc>>)>,
}

impl Verified {
    pub fn count(&self, c: Check) -> usize {
        self.rows.iter().filter(|r| r.1 == c).count()
    }

    /// Closed trades among on-time signals: (count, wins, mean return).
    pub fn closed(&self) -> (usize, usize, f64) {
        let rets: Vec<f64> = self.rows.iter().filter(|r| r.1 == Check::OnTime).filter_map(|r| r.0.ret).collect();
        let wins = rets.iter().filter(|r| **r > 0.0).count();
        let mean = if rets.is_empty() { 0.0 } else { rets.iter().sum::<f64>() / rets.len() as f64 };
        (rets.len(), wins, mean)
    }
}

/// Checks each entry against `committer`'s on-chain commitments.
pub async fn verify_entries(near: &Near, contract: &str, committer: &str, entries: Vec<RecordEntry>) -> Result<Verified> {
    let mut rows = Vec::with_capacity(entries.len());
    for e in entries {
        let c = near.view(contract, "find_commitment", json!({"committer": committer, "hash": e.hash()})).await?;
        if c.is_null() {
            rows.push((e, Check::Missing, None));
            continue;
        }
        let t = ns_to_utc(c["timestamp_ns"].as_u64().unwrap_or(0));
        let check = if t <= entry_known_at(e.as_of, e.fill_date) { Check::OnTime } else { Check::Late };
        rows.push((e, check, Some(t)));
    }
    Ok(Verified { rows })
}

pub fn print_verified(v: &Verified) {
    println!("{:<6} {:<16} {:<10} {:<9} {:<20} {:>8}  check", "symbol", "setup", "as_of", "status", "committed (UTC)", "ret");
    for (e, check, t) in &v.rows {
        let when = t.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_else(|| "-".into());
        let ret = e.ret.map(|r| format!("{:+.2}%", r * 100.0)).unwrap_or_else(|| "-".into());
        let check = match check {
            Check::OnTime => "ok (before entry)".to_string(),
            Check::Late => format!("LATE (after {})", entry_known_at(e.as_of, e.fill_date).format("%Y-%m-%d %H:%M")),
            Check::Missing => "NOT COMMITTED".to_string(),
        };
        println!("{:<6} {:<16} {:<10} {:<9} {:<20} {:>8}  {check}", e.symbol, e.setup, e.as_of, e.status, when, ret);
    }
    let (closed, wins, mean) = v.closed();
    println!(
        "\n{} committed before entry, {} late, {} never committed (of {}). Closed on-time trades: {closed}, wins {wins}, mean {:+.2}%.",
        v.count(Check::OnTime),
        v.count(Check::Late),
        v.count(Check::Missing),
        v.rows.len(),
        mean * 100.0
    );
    println!("Only on-time signals count. Timestamps are proven on-chain; returns are the seller's own paper-trade accounting.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_line_is_stable() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert_eq!(
            signal_line("NVDA", "breakout-52w", d, 201.123456, 190.0, 225.5, 20),
            "axiom-signal/v1|NVDA|breakout-52w|2026-11-06|201.1235|190.0000|225.5000|20"
        );
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(request_line("axiom-report", &["NVDA", "2026-11-06"]), "axiom-pay/v1|axiom-report|NVDA|2026-11-06");
    }

    #[test]
    fn entry_deadline_skips_weekend() {
        let fri = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert_eq!(entry_known_at(fri, None).to_string(), "2026-11-09 13:30:00 UTC");
        let fill = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        assert_eq!(entry_known_at(fri, Some(fill)).to_string(), "2026-11-10 13:30:00 UTC");
    }
}
