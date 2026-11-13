//! Agent swarm on Axiom Pay: buyer agents hire seller agents through on-chain bidding, and split purchases
//! through pools. Every step is a NEAR transaction under the wallet limits the agents' owners set.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::client::{fmt_near, parse_near, sha256_hex, u128_field};
use super::provider::{self, BidPolicy, SellerConfig};
use super::{redeem, request_line, Ctx, SEC_SERVICE};

pub struct Hired {
    pub job_id: u64,
    pub seller: String,
    pub price: u128,
    pub receipt: u64,
    pub result: Value,
}

async fn storage(ctx: &Ctx) -> Result<u128> {
    ctx.near.storage_deposit(&ctx.signer, parse_near("0.01")?).await
}

/// A bid with the seller's on-chain delivery record.
struct Ranked {
    seller: String,
    price: u128,
    endpoint: String,
    delivered: u64,
    disputed: u64,
}

/// Picks the cheapest bid from a seller with a clean record (no more disputes than deliveries); ties go to
/// the seller with more deliveries.
fn choose(bids: &[Ranked]) -> Option<&Ranked> {
    bids.iter().filter(|b| b.disputed <= b.delivered).min_by(|a, b| a.price.cmp(&b.price).then(b.delivered.cmp(&a.delivered)))
}

/// Posts `topic` as a job, waits for bids, awards the best one, collects the delivery and confirms it on-chain.
pub async fn hire(ctx: &Ctx, topic: &str, max_price: u128, wait: Duration, min_bids: usize) -> Result<Hired> {
    let me = &ctx.signer.account_id;
    let posted = ctx
        .near
        .call(&ctx.signer, &ctx.contract, "post_job", json!({"topic": topic, "max_price": max_price.to_string(), "ttl_minutes": 15}), storage(ctx).await?)
        .await
        .context("posting the job")?;
    let job_id = posted.value.as_ref().and_then(|v| v["id"].as_u64()).context("post_job returned no id")?;
    println!("  {me} posted job #{job_id} \"{topic}\" (max {}): {}", fmt_near(max_price), ctx.near.explorer_tx(&posted.hash));

    let start = Instant::now();
    let bids = loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let bids = ctx.near.view(&ctx.contract, "get_bids", json!({"job_id": job_id})).await?;
        let n = bids.as_array().map_or(0, Vec::len);
        if n >= min_bids || start.elapsed() >= wait {
            break bids;
        }
    };
    let mut ranked = Vec::new();
    for b in bids.as_array().into_iter().flatten() {
        let seller = b["seller"].as_str().unwrap_or_default().to_string();
        let st = ctx.near.view(&ctx.contract, "get_seller_stats", json!({"seller": seller})).await?;
        ranked.push(Ranked {
            price: u128_field(b, "price")?,
            endpoint: b["endpoint"].as_str().unwrap_or_default().to_string(),
            delivered: st["delivered"].as_u64().unwrap_or(0),
            disputed: st["disputed"].as_u64().unwrap_or(0),
            seller,
        });
    }
    if ranked.is_empty() {
        ctx.near.call(&ctx.signer, &ctx.contract, "cancel_job", json!({"job_id": job_id}), 0).await?;
        bail!("no bids on job #{job_id} within {}s; cancelled", wait.as_secs());
    }
    for b in &ranked {
        println!(
            "    bid {:>12} from {:<40} record: {} delivered, {} disputed",
            fmt_near(b.price),
            b.seller,
            b.delivered,
            b.disputed
        );
    }
    let Some(best) = choose(&ranked) else {
        ctx.near.call(&ctx.signer, &ctx.contract, "cancel_job", json!({"job_id": job_id}), 0).await?;
        bail!("every bidder on job #{job_id} has more disputes than deliveries; cancelled");
    };
    let awarded = ctx
        .near
        .call(&ctx.signer, &ctx.contract, "award", json!({"job_id": job_id, "seller": best.seller}), 0)
        .await
        .context("awarding the job")?;
    let receipt = awarded.value.as_ref().and_then(|v| v["id"].as_u64()).context("award returned no receipt")?;
    println!("  {me} awarded job #{job_id} to {} for {} (held in escrow): receipt #{receipt}, {}", best.seller, fmt_near(best.price), ctx.near.explorer_tx(&awarded.hash));

    let url = format!("{}/v1/jobs/{job_id}", best.endpoint.trim_end_matches('/'));
    let delivered = redeem(&reqwest::Client::new(), &url, &[], receipt).await;
    let ok = delivered.as_ref().is_ok_and(|v| !v["result"].is_null());
    let c = ctx.near.call(&ctx.signer, &ctx.contract, "confirm", json!({"job_id": job_id, "delivered": ok}), 0).await?;
    println!(
        "  {me} {} job #{job_id}: {}",
        if ok { "confirmed delivery, escrow released to the seller, for" } else { "DISPUTED, escrow refunded to the wallet, for" },
        ctx.near.explorer_tx(&c.hash)
    );
    let result = delivered?;
    Ok(Hired { job_id, seller: best.seller.clone(), price: best.price, receipt, result: result["result"].clone() })
}

/// Splits one paid call between `members`: the first opens a pool, the rest join, the last share pays the
/// seller, then every member redeems the same receipt.
pub async fn pool_buy(members: &[Ctx], seller_url: &str, service_id: &str, query: &[(&str, String)], hash_params: &[&str]) -> Result<Vec<Value>> {
    let lead = members.first().context("no pool members")?;
    let url = format!("{}/v1/{service_id}", seller_url.trim_end_matches('/'));
    let http = reqwest::Client::new();
    let first = http.get(&url).query(query).send().await.with_context(|| format!("seller {url}"))?;
    if first.status().as_u16() != 402 {
        bail!("seller answered HTTP {}: {}", first.status(), first.text().await.unwrap_or_default());
    }
    let terms: Value = first.json().await?;
    let request_hash = sha256_hex(&request_line(service_id, hash_params));
    if terms["request_hash"].as_str() != Some(request_hash.as_str()) || terms["contract"].as_str() != Some(lead.contract.as_str()) {
        bail!("seller's terms do not match our request or contract; refusing to pay");
    }
    let n = members.len() as u32;
    let memo: String = hash_params.join(" ");
    let opened = lead
        .near
        .call(
            &lead.signer,
            &lead.contract,
            "open_pool",
            json!({"service_id": service_id, "request_hash": request_hash, "memo": memo, "shares": n, "ttl_minutes": 15}),
            storage(lead).await?,
        )
        .await
        .context("opening the pool")?;
    let pool = opened.value.context("open_pool returned nothing")?;
    let pool_id = pool["id"].as_u64().context("no pool id")?;
    println!(
        "  {} opened pool #{pool_id}: {service_id} for {} split {n} ways, paid {}: {}",
        lead.signer.account_id,
        fmt_near(u128_field(&pool, "price")?),
        fmt_near(pool["paid"][0].as_str().unwrap_or("0").parse().unwrap_or(0)),
        lead.near.explorer_tx(&opened.hash)
    );
    let mut last = pool;
    for m in &members[1..] {
        let j = m
            .near
            .call(&m.signer, &m.contract, "join_pool", json!({"pool_id": pool_id}), storage(m).await?)
            .await
            .with_context(|| format!("{} joining pool #{pool_id}", m.signer.account_id))?;
        last = j.value.context("join_pool returned nothing")?;
        let paid = last["paid"].as_array().and_then(|p| p.last()).and_then(Value::as_str).unwrap_or("0").parse::<u128>().unwrap_or(0);
        println!("  {} joined pool #{pool_id}, paid {}: {}", m.signer.account_id, fmt_near(paid), m.near.explorer_tx(&j.hash));
    }
    let receipt = last["receipt"].as_u64().context("pool did not settle")?;
    println!("  pool #{pool_id} filled: seller paid in full, receipt #{receipt}");
    let mut out = Vec::new();
    for m in members {
        let v = redeem(&http, &url, query, receipt).await.with_context(|| format!("{} redeeming", m.signer.account_id))?;
        println!("  {} redeemed receipt #{receipt}", m.signer.account_id);
        out.push(v);
    }
    Ok(out)
}

pub struct SellerSpec {
    pub account: String,
    pub port: u16,
    pub prices: HashMap<String, u128>,
}

/// A collective purchase: seller URL, service, query, and the params its request hash is built from.
pub type PoolSpec = (String, String, Vec<(&'static str, String)>, Vec<String>);

pub struct SwarmPlan {
    pub sellers: Vec<SellerSpec>,
    pub buyers: Vec<String>,
    /// Job topics, assigned to buyers round-robin.
    pub topics: Vec<String>,
    pub max_price: u128,
    /// Optional collective purchase by all buyers: (seller URL, service, query, hash params).
    pub pool: Option<PoolSpec>,
    pub db_path: String,
    pub sec_user_agent: Option<String>,
}

/// Runs the demo economy: starts the seller agents (each with its own server and bidding loop), has the
/// buyer agents hire them job by job, then has all buyers pool one purchase.
pub async fn run(plan: SwarmPlan, contract: String) -> Result<()> {
    let near = super::client::Near::from_env();
    println!("Swarm on {contract} ({}): {} seller agent(s), {} buyer agent(s)\n", near.network, plan.sellers.len(), plan.buyers.len());
    // One SEC client for every seller: SEC throttles parallel loads of its ticker map from one User-Agent.
    let sec = match &plan.sec_user_agent {
        Some(ua) => Some(std::sync::Arc::new(crate::data::sec::Sec::new(ua).await.context("loading SEC ticker map")?)),
        None => None,
    };
    for s in &plan.sellers {
        let signer = super::client::Signer::load(&near.network, Some(&s.account))?;
        let cfg = SellerConfig {
            port: s.port,
            seller: s.account.clone(),
            contract: contract.clone(),
            db_path: plan.db_path.clone(),
            sec_user_agent: plan.sec_user_agent.clone(),
            sec: sec.clone(),
            bidding: Some((signer, BidPolicy { prices: s.prices.clone(), endpoint: format!("http://127.0.0.1:{}", s.port) })),
            tag: format!("[{}] ", s.account.split('.').next().unwrap_or("seller")),
        };
        tokio::spawn(async move {
            if let Err(e) = provider::serve(cfg).await {
                eprintln!("seller stopped: {e:#}");
            }
        });
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    println!();

    let buyers: Vec<Ctx> = plan.buyers.iter().map(|b| Ctx::from_env(Some(b))).collect::<Result<_>>()?;
    if buyers.is_empty() {
        bail!("no buyers");
    }
    let mut spent = 0u128;
    for (i, topic) in plan.topics.iter().enumerate() {
        let ctx = &buyers[i % buyers.len()];
        println!("\nRound {}: {} needs \"{topic}\"", i + 1, ctx.signer.account_id);
        match hire(ctx, topic, plan.max_price, Duration::from_secs(25), plan.sellers.len()).await {
            Ok(h) => {
                spent += h.price;
                let n = h.result.as_array().map_or(0, Vec::len);
                println!("  -> got {n} item(s) from {} (job #{}, receipt #{})", h.seller, h.job_id, h.receipt);
            }
            Err(e) => println!("  -> failed: {e:#}"),
        }
    }
    if let Some((url, service, query, params)) = &plan.pool {
        println!("\nCollective purchase: {} buyers split one {service} call from {url}", buyers.len());
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        match pool_buy(&buyers, url, service, query, &p).await {
            Ok(v) => println!("  -> {} buyers each received the same answer ({} bytes)", v.len(), v[0].to_string().len()),
            Err(e) => println!("  -> failed: {e:#}"),
        }
    }
    println!("\nJobs spent {} in total. Every payment, bid and delivery confirmation above is on-chain.", fmt_near(spent));
    // Let the seller tasks print their last delivery lines.
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

/// Default job prices for a seller: `sec-red-flags` at `price`.
pub fn sec_prices(price: u128) -> HashMap<String, u128> {
    HashMap::from([(SEC_SERVICE.to_string(), price)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(seller: &str, price: u128, delivered: u64, disputed: u64) -> Ranked {
        Ranked { seller: seller.into(), price, endpoint: String::new(), delivered, disputed }
    }

    #[test]
    fn chooses_cheapest_clean_seller() {
        let bids = [r("a", 30, 0, 0), r("b", 10, 1, 3), r("c", 20, 2, 0), r("d", 20, 5, 1)];
        // b is cheapest but has more disputes than deliveries; c and d tie on price, d has more deliveries.
        assert_eq!(choose(&bids).unwrap().seller, "d");
        assert!(choose(&[r("x", 1, 0, 1)]).is_none());
    }
}
