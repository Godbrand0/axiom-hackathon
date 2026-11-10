//! Public data for NEAR tokens: pools and hourly candles from GeckoTerminal, top holders from FastNEAR,
//! total supply from the token contract itself (mainnet RPC). No API keys needed.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::near::client::Near;

const GECKO: &str = "https://api.geckoterminal.com/api/v2/networks/near";
/// GeckoTerminal's free tier documents 30 calls a minute but throttles bursts well below that.
const GECKO_SPACING: Duration = Duration::from_millis(4000);

#[derive(Debug, Clone)]
pub struct Pool {
    /// GeckoTerminal pool address, e.g. `refv1-4276` (a Rhea Finance pool).
    pub address: String,
    /// NEP-141 account of the base token, e.g. `blackdragon.tkn.near`.
    pub token: String,
    pub symbol: String,
    pub dex: String,
    pub price_usd: f64,
    pub liquidity_usd: f64,
    pub volume_24h: f64,
    pub change_24h: f64,
    pub buys_24h: u64,
    pub sells_24h: u64,
    pub created: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Candle {
    /// Unix seconds at the start of the hour.
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    /// Volume in USD.
    pub v: f64,
}

pub struct Holders {
    /// Share of circulating supply (supply minus burned) held by the 10 largest holders that are not
    /// DEX/farm contracts.
    pub top10_share: f64,
    pub largest: Vec<(String, f64)>,
}

pub struct MemeData {
    http: reqwest::Client,
    last_gecko: Mutex<Option<Instant>>,
    mainnet: Near,
}

fn f(v: &Value) -> f64 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0.0),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

impl MemeData {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("http client"),
            last_gecko: Mutex::new(None),
            mainnet: Near::mainnet(),
        }
    }

    async fn gecko(&self, path: &str) -> Result<Value> {
        let url = format!("{GECKO}{path}");
        let mut last = String::new();
        for attempt in 0..5u64 {
            let wait = {
                let mut g = self.last_gecko.lock().unwrap();
                let wait = g.map(|t| GECKO_SPACING.saturating_sub(t.elapsed())).unwrap_or_default();
                *g = Some(Instant::now() + wait);
                wait
            };
            tokio::time::sleep(wait + Duration::from_secs(10 * attempt)).await;
            let resp = match self.http.get(&url).header("accept", "application/json").send().await {
                Ok(r) => r,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let status = resp.status();
            if status.as_u16() == 429 || status.is_server_error() {
                eprintln!("  GeckoTerminal HTTP {status}; backing off");
                last = format!("HTTP {status}");
                continue;
            }
            if !status.is_success() {
                bail!("GeckoTerminal {path}: HTTP {status}");
            }
            return resp.json().await.with_context(|| format!("GeckoTerminal {path}: bad JSON"));
        }
        bail!("GeckoTerminal {path}: {last}")
    }

    /// NEAR Intents entries are not AMM pools: their "reserves" are the solver network's, not a pool's,
    /// and their prices don't come from a curve anyone can trade against. Only real pools count.
    fn parse_pool(p: &Value) -> Option<Pool> {
        if p["relationships"]["dex"]["data"]["id"].as_str() == Some("near-intents") {
            return None;
        }
        let a = &p["attributes"];
        let base = p["relationships"]["base_token"]["data"]["id"].as_str()?.strip_prefix("near_")?.to_string();
        let name = a["name"].as_str().unwrap_or_default().to_string();
        Some(Pool {
            address: a["address"].as_str()?.to_string(),
            symbol: name.split(" / ").next().unwrap_or(&name).trim().to_string(),
            token: base,
            dex: p["relationships"]["dex"]["data"]["id"].as_str().unwrap_or_default().to_string(),
            price_usd: f(&a["base_token_price_usd"]),
            liquidity_usd: f(&a["reserve_in_usd"]),
            volume_24h: f(&a["volume_usd"]["h24"]),
            change_24h: f(&a["price_change_percentage"]["h24"]) / 100.0,
            buys_24h: a["transactions"]["h24"]["buys"].as_u64().unwrap_or(0),
            sells_24h: a["transactions"]["h24"]["sells"].as_u64().unwrap_or(0),
            created: a["pool_created_at"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc)),
        })
    }

    /// The most traded pools (by 24h transactions) and the newest pools.
    pub async fn active_pools(&self, pages: u32) -> Result<Vec<Pool>> {
        let mut out = Vec::new();
        for page in 1..=pages {
            let v = self.gecko(&format!("/pools?page={page}&sort=h24_tx_count_desc")).await?;
            out.extend(v["data"].as_array().into_iter().flatten().filter_map(Self::parse_pool));
        }
        let v = self.gecko("/new_pools?page=1").await?;
        out.extend(v["data"].as_array().into_iter().flatten().filter_map(Self::parse_pool));
        Ok(out)
    }

    /// Pools trading `token`, most liquid first.
    pub async fn token_pools(&self, token: &str) -> Result<Vec<Pool>> {
        let v = self.gecko(&format!("/tokens/{token}/pools?page=1")).await?;
        let mut pools: Vec<Pool> = v["data"].as_array().into_iter().flatten().filter_map(Self::parse_pool).filter(|p| p.token == token).collect();
        pools.sort_by(|a, b| b.liquidity_usd.total_cmp(&a.liquidity_usd));
        Ok(pools)
    }

    /// Hourly USD candles of the pool's base token, oldest first. `before`: only candles that start before it.
    pub async fn candles(&self, pool: &str, limit: u32, before: Option<i64>) -> Result<Vec<Candle>> {
        let mut path = format!("/pools/{pool}/ohlcv/hour?aggregate=1&limit={limit}&currency=usd&token=base");
        if let Some(b) = before {
            path.push_str(&format!("&before_timestamp={b}"));
        }
        let v = self.gecko(&path).await?;
        let mut out: Vec<Candle> = v["data"]["attributes"]["ohlcv_list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let r = r.as_array()?;
                Some(Candle { t: r.first()?.as_i64()?, o: f(&r[1]), h: f(&r[2]), l: f(&r[3]), c: f(&r[4]), v: f(&r[5]) })
            })
            .collect();
        out.sort_by_key(|c| c.t);
        out.dedup_by_key(|c| c.t);
        Ok(fill_gaps(out))
    }

    /// Top-holder concentration, from FastNEAR's holder index and the token's own `ft_total_supply`.
    pub async fn holders(&self, token: &str) -> Result<Holders> {
        let v: Value = self
            .http
            .get(format!("https://api.fastnear.com/v1/ft/{token}/top"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let supply: f64 = self.mainnet.view(token, "ft_total_supply", json!({})).await?.as_str().unwrap_or("0").parse().unwrap_or(0.0);
        if supply <= 0.0 {
            bail!("{token}: no total supply");
        }
        if v["accounts"].as_array().is_none_or(|a| a.is_empty()) {
            bail!("{token}: holder index has no accounts yet");
        }
        let mut burned = 0.0;
        let mut holders = Vec::new();
        for a in v["accounts"].as_array().into_iter().flatten() {
            let id = a["account_id"].as_str().unwrap_or_default();
            let bal: f64 = a["balance"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            if is_burn(id) {
                burned += bal;
            } else if !is_pool_contract(id) {
                holders.push((id.to_string(), bal));
            }
        }
        let circulating = (supply - burned).max(1.0);
        holders.truncate(10);
        let largest: Vec<(String, f64)> = holders.iter().map(|(id, b)| (id.clone(), b / circulating)).collect();
        Ok(Holders { top10_share: largest.iter().map(|x| x.1).sum(), largest })
    }
}

/// GeckoTerminal omits hours with no trades. Fill them with flat, zero-volume candles at the previous close,
/// so that N candles always means N hours.
pub fn fill_gaps(c: Vec<Candle>) -> Vec<Candle> {
    let mut out: Vec<Candle> = Vec::with_capacity(c.len());
    for x in c {
        if let Some(prev) = out.last().copied() {
            let mut t = prev.t + 3600;
            while t < x.t {
                out.push(Candle { t, o: prev.c, h: prev.c, l: prev.c, c: prev.c, v: 0.0 });
                t += 3600;
            }
        }
        out.push(x);
    }
    out
}

/// The all-zero implicit account has no key: tokens sent there are burned.
fn is_burn(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b == b'0')
}

/// DEX pools, farms and launchpads hold tokens for everyone, so they don't count as concentration.
pub fn is_pool_contract(id: &str) -> bool {
    ["ref-finance.near", "ref-labs.near", "rhealab.near", "meme-cooking.near", "dclv2", "veax.near", "intents.near", "bridge.near"]
        .iter()
        .any(|k| id.contains(k))
}

/// Bridged majors, stablecoins and liquid-staking tokens: not memecoins.
pub fn is_major(token: &str) -> bool {
    const MAJORS: &[&str] = &[
        "wrap.near", "usdt", "usdc", "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1", "nbtc", "eth", "zec",
        "sol", "btc", "tron", "arb-", "base-", "bsc-", "aurora", "linear-protocol", "meta-pool", "stnear", "token.v2.ref-finance.near",
        "dac17f", "a0b86991", "2260fac", "omft", "factory.bridge.near", "token.burrow.near", "xtoken.ref-finance.near",
        "token.sweat", "v2-nearx", "lst.rhealab", "token.rhealab.near", "nep245:", "npro.nearmobile.near", "dai", "frax",
        // Bridged assets from other chains (incl. tokenized stocks), not NEAR memecoins.
        "omdep.near", "-0x",
    ];
    MAJORS.iter().any(|m| token.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_missing_hours() {
        let c = |t: i64, p: f64| Candle { t, o: p, h: p, l: p, c: p, v: 1.0 };
        let f = fill_gaps(vec![c(0, 1.0), c(3 * 3600, 2.0)]);
        assert_eq!(f.iter().map(|x| x.t / 3600).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert_eq!((f[1].c, f[1].v, f[3].c), (1.0, 0.0, 2.0));
    }

    #[test]
    fn classifies_accounts() {
        assert!(is_burn(&"0".repeat(64)));
        assert!(!is_burn("alice.near"));
        assert!(is_pool_contract("v2.ref-finance.near"));
        assert!(is_pool_contract("meme-farming_011.ref-labs.near"));
        assert!(!is_pool_contract("alice.near"));
        assert!(is_major("wrap.near"));
        assert!(is_major("17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"));
        assert!(!is_major("blackdragon.tkn.near"));
        assert!(!is_major("sinu-8.smithii.near"));
        assert!(!is_major("token.0xshitzu.near"));
        assert!(is_major("bnb-0xa9ee28c80f960b889dfbd1902055218cba016f75.omdep.near"));
    }
}
