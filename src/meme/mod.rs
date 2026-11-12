//! Memecoin research on NEAR: discover active tokens, screen them for rug risk, find momentum breakouts on
//! hourly candles, and commit every signal on-chain before it is sold.
//!
//! Unlike stocks, memecoin prices are public on-chain data, so a buyer does not have to trust the seller's
//! returns: each published signal is scored from the first hourly candle that *starts after* its on-chain
//! commitment, and the buyer recomputes that from GeckoTerminal itself. The buyer also counts the seller's
//! memecoin commitments that were never revealed, so hidden losers show up.
pub mod cli;
pub mod data;

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::near::client::{sha256_hex, Near};
use data::{Candle, Holders, MemeData, Pool};

/// Active memecoin signals (committed on-chain, still inside their holding window).
pub const MEME_SIGNALS_SERVICE: &str = "meme-signals";
/// A rug-risk screen of one NEAR token.
pub const MEME_RISK_SERVICE: &str = "meme-risk";
pub const SETUP: &str = "meme-breakout";
/// Label prefix of memecoin commitments; the buyer counts them to find unrevealed signals.
pub const LABEL: &str = "axiom-meme/v1";
/// Round-trip cost assumed per trade: DEX fees (0.3% each way on Rhea) plus slippage on thin pools.
pub const COST: f64 = 0.01;
pub const MAX_HOLD_H: u32 = 72;
/// Fewer holders than this outside pools is a ghost token, whatever its concentration.
pub const MIN_WALLETS: usize = 25;

// ---------- risk ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    /// 0 (no flags) to 100.
    pub score: u32,
    pub level: String,
    pub flags: Vec<String>,
    pub top10_share: Option<f64>,
}

/// Rug-risk screen from pool and holder data. Each flag says what was seen; the score adds them up.
pub fn assess(pool: &Pool, holders: Option<&Holders>, now: DateTime<Utc>) -> Risk {
    let mut score = 0u32;
    let mut flags = Vec::new();
    let liq = pool.liquidity_usd;
    if liq < 3_000.0 {
        score += 40;
        flags.push(format!("very thin liquidity (${liq:.0}): a small sale moves the price a lot"));
    } else if liq < 10_000.0 {
        score += 25;
        flags.push(format!("thin liquidity (${liq:.0})"));
    }
    if let Some(created) = pool.created {
        let days = (now - created).num_hours() as f64 / 24.0;
        if days < 3.0 {
            score += 20;
            flags.push(format!("pool launched {days:.1} days ago"));
        } else if days < 14.0 {
            score += 5;
            flags.push(format!("pool is {days:.0} days old"));
        }
    }
    match holders {
        Some(h) if h.top10_share > 0.5 => {
            score += 25;
            flags.push(format!("top 10 holders own {:.0}% of circulating supply", h.top10_share * 100.0));
        }
        Some(h) if h.top10_share > 0.3 => {
            score += 10;
            flags.push(format!("top 10 holders own {:.0}% of circulating supply", h.top10_share * 100.0));
        }
        Some(_) => {}
        None => {
            score += 10;
            flags.push("holder data unavailable".into());
        }
    }
    // Low concentration means little when almost nobody holds the token outside its pool.
    if let Some(h) = holders.filter(|h| h.wallets < MIN_WALLETS) {
        score += 15;
        flags.push(format!("only {} wallet(s) hold it outside pools", h.wallets));
    }
    if liq > 0.0 && pool.volume_24h > 10.0 * liq {
        score += 15;
        flags.push(format!("24h volume is {:.0}x liquidity: heavy churn, possibly wash trading", pool.volume_24h / liq));
    }
    if pool.change_24h.abs() > 0.5 {
        score += 10;
        flags.push(format!("price moved {:+.0}% in 24h", pool.change_24h * 100.0));
    }
    let total = pool.buys_24h + pool.sells_24h;
    if total >= 20 && pool.sells_24h as f64 > 2.0 * pool.buys_24h as f64 {
        score += 10;
        flags.push(format!("sells outnumber buys {}:{} in 24h", pool.sells_24h, pool.buys_24h));
    }
    let score = score.min(100);
    let level = if score >= 60 { "high" } else if score >= 30 { "medium" } else { "low" };
    Risk { score, level: level.into(), flags, top10_share: holders.map(|h| h.top10_share) }
}

// ---------- signal ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
}

fn atr(c: &[Candle]) -> f64 {
    let n = c.len().saturating_sub(1).max(1);
    c.windows(2).map(|w| (w[1].h - w[1].l).max((w[1].h - w[0].c).abs()).max((w[1].l - w[0].c).abs())).sum::<f64>() / n as f64
}

/// Breakout on the closed hourly candle `i`: its close is above the previous 72 hours' high, and the last
/// 24 hours traded more than twice the average day of the 3 days before (and at least $1,000).
/// Stop: 3 hourly ATRs below entry, kept between 8% and 30% below. Target: twice the risk above entry.
pub fn breakout(c: &[Candle], i: usize) -> Option<Plan> {
    if i < 96 || i >= c.len() {
        return None;
    }
    let entry = c[i].c;
    let prior_high = c[i - 72..i].iter().map(|x| x.h).fold(f64::MIN, f64::max);
    let vol24: f64 = c[i - 23..=i].iter().map(|x| x.v).sum();
    let prior_day: f64 = c[i - 95..i - 23].iter().map(|x| x.v).sum::<f64>() / 3.0;
    if entry <= prior_high || vol24 < 1_000.0 || vol24 <= 2.0 * prior_day || entry <= 0.0 {
        return None;
    }
    let stop = (entry - 3.0 * atr(&c[i - 24..=i])).clamp(entry * 0.70, entry * 0.92);
    Some(Plan { entry, stop, target: entry + 2.0 * (entry - stop) })
}

/// Canonical text of a memecoin signal; its SHA-256 is committed on-chain.
pub fn meme_line(token: &str, pool: &str, setup: &str, signal_time: DateTime<Utc>, p: &Plan, max_hold_h: u32) -> String {
    format!(
        "{LABEL}|{token}|{pool}|{setup}|{}|{:.6e}|{:.6e}|{:.6e}|{max_hold_h}",
        signal_time.format("%Y-%m-%dT%H:%MZ"),
        p.entry,
        p.stop,
        p.target
    )
}

// ---------- outcome ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// open | target | stop | time | gapped | no-data
    pub status: String,
    pub fill: Option<f64>,
    pub exit: Option<f64>,
    /// Net of COST. For an open trade: unrealised at the last close.
    pub ret: Option<f64>,
    /// Hours from fill to exit (or to the last candle, if open).
    #[serde(default)]
    pub held_h: u32,
}

/// Scores a plan from the first candle that starts at or after `start_ts`: fill at its open, then stop or
/// target (stop first if one candle touches both), else exit at the close of the `max_hold_h`-th hour.
pub fn evaluate(c: &[Candle], start_ts: i64, p: &Plan, max_hold_h: u32) -> Outcome {
    let Some(first) = c.iter().position(|x| x.t >= start_ts) else {
        return Outcome { status: "no-data".into(), fill: None, exit: None, ret: None, held_h: 0 };
    };
    let fill = c[first].o;
    if fill >= p.target || fill <= p.stop || fill <= 0.0 {
        return Outcome { status: "gapped".into(), fill: Some(fill), exit: None, ret: None, held_h: 0 };
    }
    let net = |exit: f64| exit / fill - 1.0 - COST;
    let window = &c[first..c.len().min(first + max_hold_h as usize)];
    for (k, x) in window.iter().enumerate() {
        let held_h = k as u32 + 1;
        if x.l <= p.stop {
            let exit = x.o.min(p.stop);
            return Outcome { status: "stop".into(), fill: Some(fill), exit: Some(exit), ret: Some(net(exit)), held_h };
        }
        if x.h >= p.target {
            return Outcome { status: "target".into(), fill: Some(fill), exit: Some(p.target), ret: Some(net(p.target)), held_h };
        }
    }
    let last = window.last().unwrap().c;
    let status = if window.len() == max_hold_h as usize { "time" } else { "open" };
    Outcome {
        status: status.into(),
        fill: Some(fill),
        exit: (status == "time").then_some(last),
        ret: Some(net(last)),
        held_h: window.len() as u32,
    }
}

/// Start of the first full hour after `t`.
pub fn next_hour(t: DateTime<Utc>) -> i64 {
    (t.timestamp() / 3600 + 1) * 3600
}

// ---------- ledger ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemeSignal {
    pub token: String,
    pub symbol: String,
    pub pool: String,
    pub setup: String,
    /// Close of the signal candle.
    pub signal_time: DateTime<Utc>,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub max_hold_h: u32,
    pub risk: Risk,
    /// When the commitment landed on-chain, as recorded by the seller (buyers read it from the chain).
    pub committed_at: Option<DateTime<Utc>>,
}

impl MemeSignal {
    pub fn plan(&self) -> Plan {
        Plan { entry: self.entry, stop: self.stop, target: self.target }
    }
    pub fn line(&self) -> String {
        meme_line(&self.token, &self.pool, &self.setup, self.signal_time, &self.plan(), self.max_hold_h)
    }
    pub fn hash(&self) -> String {
        sha256_hex(&self.line())
    }
    /// End of the holding window if it started at commitment.
    pub fn window_end(&self) -> DateTime<Utc> {
        let start = self.committed_at.map(next_hour).unwrap_or(next_hour(self.signal_time));
        Utc.timestamp_opt(start, 0).unwrap() + TimeDelta::hours(self.max_hold_h as i64)
    }
}

pub struct MemeLedger {
    conn: Connection,
}

impl MemeLedger {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meme_signals (
                id           INTEGER PRIMARY KEY,
                token        TEXT NOT NULL,
                setup        TEXT NOT NULL,
                signal_time  TEXT NOT NULL,
                body         TEXT NOT NULL,
                committed_at TEXT,
                commit_tx    TEXT,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(token, setup, signal_time)
            );",
        )?;
        Ok(Self { conn })
    }

    /// Records a signal; false if it was already recorded.
    pub fn insert(&self, s: &MemeSignal) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO meme_signals (token, setup, signal_time, body) VALUES (?1, ?2, ?3, ?4)",
            params![s.token, s.setup, s.signal_time.to_rfc3339(), serde_json::to_string(s)?],
        )?;
        Ok(n == 1)
    }

    pub fn set_committed(&self, s: &MemeSignal, at: DateTime<Utc>, tx: &str) -> Result<()> {
        let mut s = s.clone();
        s.committed_at = Some(at);
        self.conn.execute(
            "UPDATE meme_signals SET committed_at = ?1, commit_tx = ?2, body = ?3 WHERE token = ?4 AND setup = ?5 AND signal_time = ?6",
            params![at.to_rfc3339(), tx, serde_json::to_string(&s)?, s.token, s.setup, s.signal_time.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<MemeSignal>> {
        let mut st = self.conn.prepare("SELECT body FROM meme_signals ORDER BY signal_time")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }
}

// ---------- scanning ----------

pub struct Screened {
    pub pool: Pool,
    pub risk: Risk,
    pub signal: Option<MemeSignal>,
}

/// The memecoins worth screening: active and new pools plus `seeds`, majors removed, one (most liquid)
/// pool per token, at least `min_liquidity` USD.
pub async fn discover(data: &MemeData, pages: u32, seeds: &[String], min_liquidity: f64) -> Result<Vec<Pool>> {
    let mut pools = data.active_pools(pages).await?;
    for s in seeds {
        match data.token_pools(s).await {
            Ok(p) => pools.extend(p.into_iter().take(1)),
            Err(e) => eprintln!("  seed {s}: {e:#}"),
        }
    }
    let mut best: HashMap<String, Pool> = HashMap::new();
    for p in pools {
        if data::is_major(&p.token) || p.liquidity_usd < min_liquidity {
            continue;
        }
        match best.get(&p.token) {
            Some(b) if b.liquidity_usd >= p.liquidity_usd => {}
            _ => {
                best.insert(p.token.clone(), p);
            }
        }
    }
    let mut out: Vec<Pool> = best.into_values().collect();
    out.sort_by(|a, b| b.volume_24h.total_cmp(&a.volume_24h));
    Ok(out)
}

/// Screens one pool: risk, plus a breakout signal on the last closed hour unless risk is high.
/// `copycats`: other tokens trading under the same ticker.
pub async fn screen(data: &MemeData, pool: Pool, copycats: usize, now: DateTime<Utc>) -> Result<Screened> {
    let holders = data.holders(&pool.token).await.ok();
    let mut risk = assess(&pool, holders.as_ref(), now);
    if copycats > 0 {
        risk.score = (risk.score + 15).min(100);
        risk.flags.push(format!("{copycats} other token(s) trade under the ticker {}: check the contract, not the name", pool.symbol));
        risk.level = (if risk.score >= 60 { "high" } else if risk.score >= 30 { "medium" } else { "low" }).into();
    }
    let candles = data.candles(&pool.address, 300, None).await?;
    // Only closed hours: the current hour's candle is still moving.
    let closed: Vec<Candle> = candles.into_iter().filter(|c| c.t + 3600 <= now.timestamp()).collect();
    let signal = match closed.len().checked_sub(1).and_then(|i| breakout(&closed, i)) {
        Some(plan) if risk.level != "high" => Some(MemeSignal {
            token: pool.token.clone(),
            symbol: pool.symbol.clone(),
            pool: pool.address.clone(),
            setup: SETUP.into(),
            signal_time: Utc.timestamp_opt(closed.last().unwrap().t + 3600, 0).unwrap(),
            entry: plan.entry,
            stop: plan.stop,
            target: plan.target,
            max_hold_h: MAX_HOLD_H,
            risk: risk.clone(),
            committed_at: None,
        }),
        _ => None,
    };
    Ok(Screened { pool, risk, signal })
}

/// A standalone risk report for one token (its most liquid pool).
pub async fn risk_report(data: &MemeData, token: &str) -> Result<serde_json::Value> {
    let pool = data.token_pools(token).await?.into_iter().next().with_context(|| format!("no pools for {token}"))?;
    let holders = data.holders(token).await.ok();
    let risk = assess(&pool, holders.as_ref(), Utc::now());
    Ok(json!({
        "token": token,
        "symbol": pool.symbol,
        "pool": pool.address,
        "dex": pool.dex,
        "price_usd": pool.price_usd,
        "liquidity_usd": pool.liquidity_usd,
        "volume_24h_usd": pool.volume_24h,
        "change_24h": pool.change_24h,
        "buys_24h": pool.buys_24h,
        "sells_24h": pool.sells_24h,
        "pool_created": pool.created,
        "risk": risk,
        "largest_holders": holders.map(|h| h.largest.into_iter().map(|(a, s)| json!({"account": a, "share": s})).collect::<Vec<_>>()),
        "as_of": Utc::now(),
        "note": "Risk screen from public data (GeckoTerminal, FastNEAR, the token contract). Not investment advice.",
    }))
}

// ---------- backtest ----------

/// Runs the breakout rule over a pool's available hourly history, one position at a time.
pub fn backtest_candles(c: &[Candle]) -> Vec<Outcome> {
    let mut out = Vec::new();
    let mut i = 96;
    while i + 1 < c.len() {
        if let Some(p) = breakout(c, i) {
            let o = evaluate(&c[i + 1..], c[i].t + 3600, &p, MAX_HOLD_H);
            let held = o.held_h as usize;
            if o.status != "open" {
                out.push(o);
            }
            i += held + 1;
        } else {
            i += 1;
        }
    }
    out
}

// ---------- published record and verification ----------

/// The free track record: signals whose holding window has ended, with everything needed to recompute
/// their hash and their outcome.
pub fn published(all: &[MemeSignal], now: DateTime<Utc>) -> Vec<MemeSignal> {
    all.iter().filter(|s| s.committed_at.is_some() && s.window_end() <= now).cloned().collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct MemeCheck {
    pub signal: MemeSignal,
    /// From the chain, under the seller's account.
    pub committed_at: Option<DateTime<Utc>>,
    /// Commitment delay after the signal candle closed, in minutes.
    pub delay_min: Option<i64>,
    /// Recomputed by the buyer from public candles, starting after the commitment.
    pub outcome: Option<Outcome>,
}

pub struct MemeVerified {
    pub rows: Vec<MemeCheck>,
    /// The seller's memecoin commitments older than a holding window that match no published signal.
    pub unrevealed: usize,
    pub total_commitments: usize,
}

/// Checks a seller's published memecoin record against its on-chain commitments and recomputes each
/// outcome from GeckoTerminal candles.
pub async fn verify(near: &Near, contract: &str, committer: &str, entries: Vec<MemeSignal>, data: &MemeData) -> Result<MemeVerified> {
    let n = near.view(contract, "commitment_count", json!({})).await?.as_u64().unwrap_or(0);
    let mut mine = Vec::new();
    let mut from = 0;
    while from < n {
        let page = near.view(contract, "get_commitments", json!({"from": from, "limit": 100})).await?;
        for c in page.as_array().into_iter().flatten() {
            if c["committer"].as_str() == Some(committer) && c["label"].as_str().is_some_and(|l| l.starts_with(LABEL)) {
                mine.push((c["hash"].as_str().unwrap_or_default().to_string(), c["timestamp_ns"].as_u64().unwrap_or(0)));
            }
        }
        from += 100;
    }
    let by_hash: HashMap<&str, u64> = mine.iter().map(|(h, t)| (h.as_str(), *t)).collect();
    let mut rows = Vec::new();
    let mut matched = 0;
    for s in entries {
        let committed_at = by_hash.get(s.hash().as_str()).map(|ns| Utc.timestamp_opt((ns / 1_000_000_000) as i64, 0).unwrap());
        let outcome = match committed_at {
            Some(t) => {
                matched += 1;
                let start = next_hour(t);
                let end = start + 3600 * (s.max_hold_h as i64 + 1);
                let candles = data.candles(&s.pool, s.max_hold_h + 2, Some(end)).await?;
                Some(evaluate(&candles, start, &s.plan(), s.max_hold_h))
            }
            None => None,
        };
        rows.push(MemeCheck { delay_min: committed_at.map(|t| (t - s.signal_time).num_minutes()), committed_at, outcome, signal: s });
    }
    let cutoff = (Utc::now() - TimeDelta::hours(MAX_HOLD_H as i64 + 2)).timestamp_nanos_opt().unwrap_or(0) as u64;
    let old = mine.iter().filter(|(_, t)| *t < cutoff).count();
    Ok(MemeVerified { unrevealed: old.saturating_sub(matched), total_commitments: mine.len(), rows })
}

pub fn print_verified(v: &MemeVerified) {
    println!("{:<12} {:<17} {:>10} {:>14} {:>8} {:>8}", "token", "signal (UTC)", "delay", "outcome", "ret", "seller");
    let mut rets = Vec::new();
    for r in &v.rows {
        let delay = r.delay_min.map(|m| format!("{m} min")).unwrap_or_else(|| "NOT COMMITTED".into());
        let (status, ret) = match &r.outcome {
            Some(o) => (o.status.clone(), o.ret.map(|x| format!("{:+.1}%", x * 100.0)).unwrap_or_else(|| "-".into())),
            None => ("-".into(), "-".into()),
        };
        if let Some(o) = &r.outcome {
            if o.status != "open" {
                rets.extend(o.ret);
            }
        }
        println!("{:<12} {:<17} {:>10} {:>14} {:>8}", r.signal.symbol, r.signal.signal_time.format("%Y-%m-%d %H:%M"), delay, status, ret);
    }
    let wins = rets.iter().filter(|r| **r > 0.0).count();
    let mean = if rets.is_empty() { 0.0 } else { rets.iter().sum::<f64>() / rets.len() as f64 };
    println!(
        "\n{} published, {} committed on-chain; {} closed trades recomputed from public candles: {} wins, mean {:+.1}% net of {:.0}% costs.",
        v.rows.len(),
        v.rows.iter().filter(|r| r.committed_at.is_some()).count(),
        rets.len(),
        wins,
        mean * 100.0,
        COST * 100.0
    );
    println!(
        "Seller has {} memecoin commitment(s) on-chain; {} older than a holding window were never revealed{}.",
        v.total_commitments,
        v.unrevealed,
        if v.unrevealed > 0 { " (hidden signals: treat the record as incomplete)" } else { "" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, price: f64, vol: f64) -> Vec<Candle> {
        (0..n).map(|i| Candle { t: i as i64 * 3600, o: price, h: price * 1.01, l: price * 0.99, c: price, v: vol }).collect()
    }

    #[test]
    fn breakout_needs_new_high_and_volume() {
        let mut c = flat(120, 1.0, 100.0);
        assert!(breakout(&c, 119).is_none());
        // New high but no volume surge.
        c[119].c = 1.2;
        c[119].h = 1.2;
        assert!(breakout(&c, 119).is_none());
        // Volume surge in the last 24h.
        for x in &mut c[96..120] {
            x.v = 1_000.0;
        }
        let p = breakout(&c, 119).unwrap();
        assert_eq!(p.entry, 1.2);
        assert!(p.stop >= 1.2 * 0.70 && p.stop <= 1.2 * 0.92);
        assert!((p.target - (p.entry + 2.0 * (p.entry - p.stop))).abs() < 1e-12);
    }

    #[test]
    fn evaluate_paths() {
        let p = Plan { entry: 1.0, stop: 0.9, target: 1.2 };
        let mut c = flat(10, 1.0, 1.0);
        c[3].h = 1.25;
        assert_eq!(evaluate(&c, 0, &p, 72).status, "target");
        let o = evaluate(&c, 0, &p, 72);
        assert!((o.ret.unwrap() - (0.2 - COST)).abs() < 1e-12);
        c[2].l = 0.85; // stop comes first
        assert_eq!(evaluate(&c, 0, &p, 72).status, "stop");
        let c = flat(10, 1.0, 1.0);
        assert_eq!(evaluate(&c, 0, &p, 5).status, "time");
        assert_eq!(evaluate(&c, 0, &p, 72).status, "open");
        assert_eq!(evaluate(&c, 99_999, &p, 72).status, "no-data");
        let mut g = flat(3, 1.3, 1.0);
        g[0].o = 1.3;
        assert_eq!(evaluate(&g, 0, &p, 72).status, "gapped");
    }

    #[test]
    fn line_is_stable() {
        let t = Utc.with_ymd_and_hms(2026, 11, 10, 9, 0, 0).unwrap();
        let p = Plan { entry: 6.461657e-9, stop: 5.9e-9, target: 7.58e-9 };
        assert_eq!(
            meme_line("blackdragon.tkn.near", "refv1-4276", SETUP, t, &p, 72),
            "axiom-meme/v1|blackdragon.tkn.near|refv1-4276|meme-breakout|2026-11-10T09:00Z|6.461657e-9|5.900000e-9|7.580000e-9|72"
        );
        assert_eq!(next_hour(t), t.timestamp() + 3600);
        assert_eq!(next_hour(t + TimeDelta::minutes(5)), t.timestamp() + 3600);
    }

    #[test]
    fn risk_flags_add_up() {
        let pool = Pool {
            address: "x".into(),
            token: "x.near".into(),
            symbol: "X".into(),
            dex: "rhea".into(),
            price_usd: 1.0,
            liquidity_usd: 2_000.0,
            volume_24h: 50_000.0,
            change_24h: 0.9,
            buys_24h: 10,
            sells_24h: 40,
            created: Some(Utc::now() - TimeDelta::hours(12)),
        };
        let h = Holders { top10_share: 0.7, largest: vec![], wallets: 500 };
        let r = assess(&pool, Some(&h), Utc::now());
        assert_eq!(r.level, "high");
        assert_eq!(r.score, 100);
        assert_eq!(r.flags.len(), 6);
    }

    #[test]
    fn flags_ghost_tokens() {
        let pool = Pool {
            address: "x".into(),
            token: "x.near".into(),
            symbol: "X".into(),
            dex: "rhea".into(),
            price_usd: 1.0,
            liquidity_usd: 50_000.0,
            volume_24h: 1_000.0,
            change_24h: 0.0,
            buys_24h: 5,
            sells_24h: 5,
            created: Some(Utc::now() - TimeDelta::days(60)),
        };
        // Nearly all supply in the pool: concentration looks tiny, but only two wallets hold any.
        let h = Holders { top10_share: 0.003, largest: vec![], wallets: 2 };
        let r = assess(&pool, Some(&h), Utc::now());
        assert_eq!((r.score, r.flags.len()), (15, 1));
        assert!(r.flags[0].contains("only 2 wallet"));
    }
}
