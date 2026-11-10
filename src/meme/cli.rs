//! `axiom meme ...`: memecoin research, selling and verification.
use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::Subcommand;
use serde_json::{json, Value};

use super::data::MemeData;
use super::{
    backtest_candles, discover, print_verified, published, risk_report, screen, verify, MemeLedger, MemeSignal, LABEL, MEME_SIGNALS_SERVICE,
};
use crate::near::client::Near;
use crate::near::{self, Ctx};

#[derive(Subcommand)]
pub enum MemeCmd {
    /// Screen active NEAR memecoins for rug risk and momentum breakouts; record (and with --near, commit) signals.
    Scan {
        /// Pages of GeckoTerminal's most-traded NEAR pools to scan (20 per page), plus the newest pools.
        #[arg(long, default_value_t = 3)]
        pages: u32,
        #[arg(long, default_value_t = 2000.0)]
        min_liquidity: f64,
        /// Extra tokens to always screen, one NEP-141 account per line.
        #[arg(long, default_value = "config/memecoins.txt")]
        seeds: String,
        /// Commit each new signal on-chain as the --as account before it can be sold.
        #[arg(long)]
        near: bool,
    },
    /// Rug-risk report for one token, e.g. `blackdragon.tkn.near`. With --seller, buy it from a seller instead.
    Risk {
        token: String,
        #[arg(long)]
        seller: Option<String>,
    },
    /// Run the breakout rule over the available hourly history of today's active memecoins.
    Backtest {
        #[arg(long, default_value_t = 3)]
        pages: u32,
        #[arg(long, default_value_t = 2000.0)]
        min_liquidity: f64,
        #[arg(long, default_value = "config/memecoins.txt")]
        seeds: String,
    },
    /// Check a seller's memecoin record: on-chain commitments, unrevealed signals, and outcomes recomputed
    /// from public candles. Free; no key needed.
    Verify {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        seller: String,
    },
    /// Buyer agent: verify the seller's memecoin record, then buy its active signals if it passes.
    Shop {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        seller: String,
        /// Refuse unless at least this many published signals verify on-chain.
        #[arg(long, default_value_t = 1)]
        min_verified: usize,
    },
    /// Seller: write the free memecoin track record to a JSON file (e.g. for the website).
    ExportRecord {
        #[arg(long, default_value = "web/public/meme-record.json")]
        out: String,
    },
}

fn seeds(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(|l| l.split('#').next().unwrap_or("").trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

fn pct(x: f64) -> String {
    format!("{:+.1}%", x * 100.0)
}

pub async fn run(as_account: Option<&str>, cmd: MemeCmd) -> Result<()> {
    let db = std::env::var("AXIOM_DB").unwrap_or_else(|_| "axiom.db".into());
    let data = MemeData::new();
    match cmd {
        MemeCmd::Scan { pages, min_liquidity, seeds: seeds_path, near: commit } => {
            let ctx = if commit { Some(Ctx::from_env(as_account)?) } else { None };
            let ledger = MemeLedger::open(&db)?;
            let now = Utc::now();
            let pools = discover(&data, pages, &seeds(&seeds_path), min_liquidity).await?;
            println!("Screening {} NEAR memecoin(s) with at least ${min_liquidity:.0} liquidity ({})\n", pools.len(), now.format("%Y-%m-%d %H:%M UTC"));
            println!("{:<12} {:>10} {:>10} {:>8} {:>6}  {:<6} signal", "token", "liquidity", "vol 24h", "24h", "top10", "risk");
            let mut tickers: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for p in &pools {
                *tickers.entry(p.symbol.to_uppercase()).or_default() += 1;
            }
            let mut new = Vec::new();
            for pool in pools {
                let copycats = tickers[&pool.symbol.to_uppercase()] - 1;
                let s = match screen(&data, pool, copycats, now).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("  skipped: {e:#}");
                        continue;
                    }
                };
                let top10 = s.risk.top10_share.map(|x| format!("{:.0}%", x * 100.0)).unwrap_or_else(|| "?".into());
                let sig = match &s.signal {
                    Some(m) => format!("BREAKOUT entry {:.4e} stop {:.4e} target {:.4e}", m.entry, m.stop, m.target),
                    None => "-".into(),
                };
                println!(
                    "{:<12} {:>10} {:>10} {:>8} {:>6}  {:<3}{:>3}  {sig}",
                    s.pool.symbol.chars().take(12).collect::<String>(),
                    format!("${:.0}", s.pool.liquidity_usd),
                    format!("${:.0}", s.pool.volume_24h),
                    pct(s.pool.change_24h),
                    top10,
                    &s.risk.level[..3.min(s.risk.level.len())],
                    s.risk.score
                );
                for f in &s.risk.flags {
                    println!("{:>14}- {f}", "");
                }
                if let Some(m) = s.signal {
                    if ledger.insert(&m)? {
                        new.push(m);
                    }
                }
            }
            println!("\n{} new signal(s) recorded.", new.len());
            if let Some(ctx) = &ctx {
                for m in &new {
                    let label = format!("{LABEL} {}", m.signal_time.format("%Y-%m-%d"));
                    let deposit = ctx.near.storage_deposit(&ctx.signer, crate::near::client::parse_near("0.01")?).await?;
                    match ctx.near.call(&ctx.signer, &ctx.contract, "commit", json!({"hash": m.hash(), "label": label}), deposit).await {
                        Ok(r) => {
                            let ns = r.value.as_ref().and_then(|v| v["timestamp_ns"].as_u64()).unwrap_or(0);
                            let at = chrono::DateTime::<Utc>::from_timestamp((ns / 1_000_000_000) as i64, 0).unwrap_or_else(Utc::now);
                            ledger.set_committed(m, at, &r.hash)?;
                            println!("  committed {} {}: {}", m.symbol, m.signal_time.format("%H:%M"), ctx.near.explorer_tx(&r.hash));
                        }
                        Err(e) => eprintln!("  commit failed for {}: {e:#} (not for sale until committed)", m.symbol),
                    }
                }
            }
        }
        MemeCmd::Risk { token, seller } => {
            let r = match seller {
                Some(url) => {
                    let ctx = Ctx::from_env(as_account)?;
                    let date = Utc::now().date_naive().to_string();
                    near::purchase(&ctx, &url, super::MEME_RISK_SERVICE, &[("token", token.clone()), ("date", date.clone())], &[&token, &date]).await?
                }
                None => risk_report(&data, &token).await?,
            };
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        MemeCmd::Backtest { pages, min_liquidity, seeds: seeds_path } => {
            let pools = discover(&data, pages, &seeds(&seeds_path), min_liquidity).await?;
            println!("Breakout rule over each token's last ~1000 hourly candles ({} tokens; costs {:.0}% per trade)\n", pools.len(), super::COST * 100.0);
            let mut all = Vec::new();
            for p in pools {
                let c = match data.candles(&p.address, 1000, None).await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("  {}: {e:#}", p.symbol);
                        continue;
                    }
                };
                let trades = backtest_candles(&c);
                let rets: Vec<f64> = trades.iter().filter_map(|t| t.ret).collect();
                let mean = if rets.is_empty() { 0.0 } else { rets.iter().sum::<f64>() / rets.len() as f64 };
                println!(
                    "{:<12} {:>5} hours  {:>3} trades  {:>3} wins  mean {:>7}",
                    p.symbol.chars().take(12).collect::<String>(),
                    c.len(),
                    rets.len(),
                    rets.iter().filter(|r| **r > 0.0).count(),
                    pct(mean)
                );
                all.extend(rets);
            }
            let mean = if all.is_empty() { 0.0 } else { all.iter().sum::<f64>() / all.len() as f64 };
            println!(
                "\nAll: {} trades, {} wins, mean {} per trade. Survivorship bias: only tokens still active today are tested.",
                all.len(),
                all.iter().filter(|r| **r > 0.0).count(),
                pct(mean)
            );
        }
        MemeCmd::Verify { seller } => {
            let (committer, contract, entries) = fetch_record(&seller).await?;
            println!("Seller {committer}, commitments on {contract}\n");
            let v = verify(&Near::from_env(), &contract, &committer, entries, &data).await?;
            print_verified(&v);
        }
        MemeCmd::Shop { seller, min_verified } => {
            let ctx = Ctx::from_env(as_account)?;
            let (committer, contract, entries) = fetch_record(&seller).await?;
            if contract != ctx.contract {
                bail!("seller uses contract {contract}, we use {}", ctx.contract);
            }
            let v = verify(&ctx.near, &contract, &committer, entries, &data).await?;
            print_verified(&v);
            let ok = v.rows.iter().filter(|r| r.committed_at.is_some()).count();
            if ok < min_verified {
                println!("\nDECISION: not buying. Only {ok} memecoin signal(s) verified on-chain (need {min_verified}).");
                return Ok(());
            }
            if v.unrevealed > 0 {
                println!("\nDECISION: not buying. {} committed signal(s) were never revealed.", v.unrevealed);
                return Ok(());
            }
            let hour = Utc::now().format("%Y-%m-%dT%H").to_string();
            println!("\nDECISION: buying active memecoin signals for {hour}.");
            let body = near::purchase(&ctx, &seller, MEME_SIGNALS_SERVICE, &[("hour", hour.clone())], &[&hour]).await?;
            let sigs = body["signals"].as_array().cloned().unwrap_or_default();
            let mut confirmed = 0;
            for s in &sigs {
                let m: MemeSignal = serde_json::from_value(s.clone()).context("bad signal from seller")?;
                let found = ctx.near.view(&contract, "find_commitment", json!({"committer": committer, "hash": m.hash()})).await?;
                confirmed += usize::from(!found.is_null());
                println!(
                    "  {:<10} entry {:.4e} stop {:.4e} target {:.4e}  risk {} ({})  {}",
                    m.symbol,
                    m.entry,
                    m.stop,
                    m.target,
                    m.risk.score,
                    m.risk.level,
                    if found.is_null() { "NOT COMMITTED" } else { "committed on-chain" }
                );
            }
            println!("{} signal(s), {confirmed} committed on-chain.", sigs.len());
        }
        MemeCmd::ExportRecord { out } => {
            let seller = as_account.map(str::to_string).or_else(|| std::env::var("NEAR_ACCOUNT_ID").ok()).context("pass --as <seller>")?;
            let all = MemeLedger::open(&db)?.all()?;
            let entries = published(&all, Utc::now());
            let body = json!({
                "seller": seller, "committer": seller, "contract": near::contract_id()?, "network": Near::from_env().network,
                "exported_at": Utc::now(), "entries": entries,
                "active_signals": all.iter().filter(|s| s.committed_at.is_some()).count() - entries.len(),
            });
            std::fs::write(&out, serde_json::to_string_pretty(&body)?)?;
            println!("Wrote {} published memecoin signal(s) to {out}", entries.len());
        }
    }
    Ok(())
}

async fn fetch_record(seller: &str) -> Result<(String, String, Vec<MemeSignal>)> {
    let v: Value = reqwest::get(format!("{}/v1/meme-track-record", seller.trim_end_matches('/'))).await?.error_for_status()?.json().await?;
    let committer = v["committer"].as_str().context("record has no committer")?.to_string();
    let contract = v["contract"].as_str().context("record has no contract")?.to_string();
    let entries: Vec<MemeSignal> = serde_json::from_value(v["entries"].clone())?;
    Ok((committer, contract, entries))
}
