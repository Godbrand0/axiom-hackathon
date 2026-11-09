//! Axiom Pay on NEAR: the agent buys evidence from other agents under an owner-set policy, and commits
//! every signal on-chain before its outcome is known. See `contract/` for the smart contract.
pub mod client;
pub mod provider;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc, Weekday};
use serde_json::{json, Value};

use crate::models::{Filing, Signal};
use crate::store::StoredSignal;
use client::{fmt_near, sha256_hex, u128_field, Near, Signer};

/// Service id under which the provider sells SEC filings with 8-K red flags.
pub const SEC_SERVICE: &str = "sec-red-flags";

/// What the agent needs to act on-chain: RPC, its key, and the Axiom Pay contract.
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

/// The request a paid call is bound to. Buyer and provider both compute it; the receipt stores its hash.
pub fn request_line(service_id: &str, symbol: &str, since: NaiveDate) -> String {
    format!("axiom-pay/v1|{service_id}|{}|{since}", symbol.to_uppercase())
}

/// Canonical text of a signal. Its SHA-256 is what gets committed on-chain.
/// Fixed precision so the same signal always hashes the same, from memory or from the database.
pub fn signal_line(symbol: &str, setup: &str, as_of: NaiveDate, entry: f64, stop: f64, target: f64, max_hold: usize) -> String {
    format!("axiom-signal/v1|{symbol}|{setup}|{as_of}|{entry:.4}|{stop:.4}|{target:.4}|{max_hold}")
}

pub fn signal_hash(s: &Signal) -> String {
    sha256_hex(&signal_line(&s.symbol, &s.setup, s.as_of, s.entry, s.stop, s.target, s.max_hold_days))
}

pub fn stored_hash(s: &StoredSignal) -> String {
    sha256_hex(&signal_line(&s.symbol, &s.setup, s.as_of, s.entry, s.stop, s.target, s.max_hold))
}

/// Commits a signal hash; returns (hash, tx hash).
pub async fn commit_signal(ctx: &Ctx, hash: &str, label: &str) -> Result<String> {
    let r = ctx.near.call(&ctx.signer, &ctx.contract, "commit", json!({"hash": hash, "label": label}), 0).await?;
    Ok(r.hash)
}

/// Buys recent SEC filings for `symbol` from a provider agent, x402-style:
/// ask -> 402 with terms -> check terms against our own request hash and the on-chain service -> pay -> redeem.
pub async fn buy_filings(ctx: &Ctx, provider_url: &str, symbol: &str, since: NaiveDate) -> Result<Vec<Filing>> {
    let http = reqwest::Client::new();
    let url = format!("{}/v1/{SEC_SERVICE}", provider_url.trim_end_matches('/'));
    let query = [("symbol", symbol.to_string()), ("since", since.to_string())];

    let first = http.get(&url).query(&query).send().await.with_context(|| format!("provider {url}"))?;
    if first.status().is_success() {
        return Ok(first.json().await?); // free
    }
    if first.status().as_u16() != 402 {
        bail!("provider answered HTTP {}: {}", first.status(), first.text().await.unwrap_or_default());
    }
    let terms: Value = first.json().await.context("402 without JSON terms")?;
    let request_hash = sha256_hex(&request_line(SEC_SERVICE, symbol, since));
    if terms["request_hash"].as_str() != Some(request_hash.as_str()) {
        bail!("provider quoted a different request hash; refusing to pay");
    }
    if terms["contract"].as_str() != Some(ctx.contract.as_str()) {
        bail!("provider wants payment on {}, not our contract {}", terms["contract"], ctx.contract);
    }
    let svc = ctx
        .near
        .view(&ctx.contract, "get_service", json!({"service_id": SEC_SERVICE}))
        .await?;
    if svc.is_null() {
        bail!("service {SEC_SERVICE} is not registered on {}", ctx.contract);
    }
    if !svc["approved"].as_bool().unwrap_or(false) {
        bail!("service {SEC_SERVICE} is not approved by the owner (run `axiom near approve {SEC_SERVICE}` as owner)");
    }
    let price = u128_field(&svc, "price")?;

    let paid = ctx
        .near
        .call(&ctx.signer, &ctx.contract, "pay", json!({"service_id": SEC_SERVICE, "request_hash": request_hash, "memo": format!("{symbol} since {since}")}), 0)
        .await
        .context("on-chain payment")?;
    let receipt = paid.value.as_ref().and_then(|v| v["id"].as_u64()).context("pay returned no receipt id")?;
    eprintln!("    paid {} to {} for {symbol} filings: receipt #{receipt}, {}", fmt_near(price), svc["provider"].as_str().unwrap_or("?"), ctx.near.explorer_tx(&paid.hash));

    redeem(&http, &url, &query, receipt).await
}

/// Redeems a paid receipt. Retries while the receipt is not yet final on the provider's RPC and while the
/// provider has a transient upstream failure (it un-redeems the receipt in that case, so retrying is safe).
async fn redeem(http: &reqwest::Client, url: &str, query: &[(&str, String)], receipt: u64) -> Result<Vec<Filing>> {
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
    bail!("provider did not honour receipt #{receipt}: {last} (it can be redeemed for 15 minutes after payment)")
}

/// Earliest moment the next session's open could be known: 13:30 UTC (09:30 EDT) on the next weekday.
/// A commitment before this cannot have used the fill price.
fn entry_known_at(as_of: NaiveDate, fill: Option<NaiveDate>) -> DateTime<Utc> {
    let mut d = fill.unwrap_or(as_of + TimeDelta::days(1));
    while fill.is_none() && matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
        d += TimeDelta::days(1);
    }
    d.and_hms_opt(13, 30, 0).unwrap().and_utc()
}

/// Checks every logged signal against its on-chain commitment.
pub async fn verify(near: &Near, contract: &str, signals: &[StoredSignal]) -> Result<()> {
    if signals.is_empty() {
        println!("No signals in the ledger yet. Run `axiom scan --near` first.");
        return Ok(());
    }
    let (mut ok, mut late, mut missing) = (0, 0, 0);
    println!("{:<6} {:<16} {:<10} {:<9} {:<26} result", "symbol", "setup", "as_of", "status", "committed (UTC)");
    for s in signals {
        let hash = stored_hash(s);
        let c = near.view(contract, "find_commitment", json!({"hash": hash})).await?;
        let (when, verdict) = if c.is_null() {
            missing += 1;
            ("-".to_string(), "NOT COMMITTED".to_string())
        } else {
            let ns = c["timestamp_ns"].as_u64().unwrap_or(0);
            let t = DateTime::<Utc>::from_timestamp((ns / 1_000_000_000) as i64, 0).unwrap_or_default();
            let deadline = entry_known_at(s.as_of, s.fill_date);
            if t <= deadline {
                ok += 1;
                (t.format("%Y-%m-%d %H:%M:%S").to_string(), format!("ok, block {}", c["block_height"]))
            } else {
                late += 1;
                (t.format("%Y-%m-%d %H:%M:%S").to_string(), format!("LATE (after {})", deadline.format("%Y-%m-%d %H:%M")))
            }
        };
        let ret = s.ret.map(|r| format!(" ret {:+.2}%", r * 100.0)).unwrap_or_default();
        println!("{:<6} {:<16} {:<10} {:<9} {:<26} {verdict}{ret}", s.symbol, s.setup, s.as_of, s.status, when);
    }
    println!(
        "\n{ok} committed before entry, {late} committed late, {missing} never committed (of {}). Contract: {contract}",
        signals.len()
    );
    println!("Only the first group can be counted as an out-of-sample track record.");
    Ok(())
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
    }

    #[test]
    fn entry_deadline_skips_weekend() {
        let fri = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert_eq!(entry_known_at(fri, None).to_string(), "2026-11-09 13:30:00 UTC");
        let fill = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        assert_eq!(entry_known_at(fri, Some(fill)).to_string(), "2026-11-10 13:30:00 UTC");
    }
}
