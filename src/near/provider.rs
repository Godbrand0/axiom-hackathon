//! Seller server: sells services per call against Axiom Pay receipts, and delivers jobs it won.
//!
//! It serves whichever of these services the seller account has registered on-chain:
//! * `axiom-signals` / `axiom-report`: Axiom's own fresh research (the business).
//! * `sec-red-flags`: SEC filings with 8-K red-flag codes (an evidence seller Axiom itself buys from).
//!
//! `/v1/track-record` is free: every signal whose entry is known, with the fields needed to recompute
//! its hash, so a buyer can check the seller's on-chain commitments before paying for fresh ones.
//! A paid request is served only against a receipt that names this seller, this service and the hash of this
//! exact request, is under 15 minutes old, and has not been redeemed before. A receipt that settled a pool
//! of N buyers can be redeemed N times.
//!
//! Jobs: with bidding enabled the server watches the job board, bids on topics it can deliver, and serves
//! `GET /v1/jobs/<id>?receipt=<n>` to the buyer that awarded it the job.
#![allow(clippy::result_large_err)] // handlers return axum Responses as errors
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{NaiveDate, TimeDelta, Utc};
use serde_json::{json, Value};

use super::client::{fmt_near, sha256_hex, u128_field, Near, Signer};
use super::{entry_known_at, job_request_hash, request_line, RecordEntry, REPORT_SERVICE, SEC_SERVICE, SIGNALS_SERVICE};
use crate::data::sec::Sec;
use crate::store::Store;

const MAX_RECEIPT_AGE_SECS: u64 = 15 * 60;

/// How a seller bids on jobs: a fixed price per job kind (the topic's first field).
#[derive(Clone, Default)]
pub struct BidPolicy {
    pub prices: HashMap<String, u128>,
    /// Public base URL buyers fetch deliveries from.
    pub endpoint: String,
}

pub struct SellerConfig {
    pub port: u16,
    pub seller: String,
    pub contract: String,
    pub db_path: String,
    pub sec_user_agent: Option<String>,
    /// An already-loaded SEC client to share (several sellers in one process load it once).
    pub sec: Option<Arc<Sec>>,
    /// Bid on jobs automatically (needs the seller's key).
    pub bidding: Option<(Signer, BidPolicy)>,
    /// Prefix for log lines, so several sellers in one process can be told apart.
    pub tag: String,
}

struct AppState {
    near: Near,
    contract: String,
    seller: String,
    db_path: String,
    sec: Option<Arc<Sec>>,
    /// service id -> description, for the services this seller registered.
    selling: HashMap<String, String>,
    /// receipt id -> times redeemed.
    redeemed: Mutex<HashMap<u64, u32>>,
    tag: String,
}

type Q = Query<HashMap<String, String>>;

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

pub async fn serve(cfg: SellerConfig) -> Result<()> {
    let near = Near::from_env();
    let services = near.view(&cfg.contract, "list_services", json!({})).await?;
    let mut selling = HashMap::new();
    for pair in services.as_array().into_iter().flatten() {
        if pair[1]["provider"].as_str() == Some(cfg.seller.as_str()) {
            let id = pair[0].as_str().unwrap_or_default().to_string();
            println!("{}selling {id:<16} {}/call", cfg.tag, fmt_near(u128_field(&pair[1], "price")?));
            selling.insert(id, pair[1]["description"].as_str().unwrap_or_default().to_string());
        }
    }
    if selling.is_empty() && cfg.bidding.is_none() {
        anyhow::bail!(
            "{} has no services on {}; run `axiom near register --service <id> --price <NEAR> --as {}`, or bid on jobs with --bid",
            cfg.seller,
            cfg.contract,
            cfg.seller
        );
    }
    let wants_sec = selling.contains_key(SEC_SERVICE)
        || cfg.bidding.as_ref().is_some_and(|(_, p)| p.prices.contains_key(SEC_SERVICE));
    let sec = match (wants_sec, cfg.sec, cfg.sec_user_agent) {
        (false, _, _) => None,
        (true, Some(shared), _) => Some(shared),
        (true, None, Some(ua)) => Some(Arc::new(Sec::new(&ua).await.context("loading SEC ticker map")?)),
        (true, None, None) => anyhow::bail!("selling {SEC_SERVICE} needs SEC_USER_AGENT"),
    };
    let state = Arc::new(AppState {
        near,
        contract: cfg.contract,
        seller: cfg.seller,
        db_path: cfg.db_path,
        sec,
        selling,
        redeemed: Mutex::new(HashMap::new()),
        tag: cfg.tag,
    });
    if let Some((signer, policy)) = cfg.bidding {
        for (kind, p) in &policy.prices {
            println!("{}bidding on {kind} jobs at {}", state.tag, fmt_near(*p));
        }
        tokio::spawn(bid_loop(state.clone(), signer, policy));
    }
    let app = Router::new()
        .route("/", get(info))
        .route("/v1/track-record", get(track_record))
        .route(&format!("/v1/{SIGNALS_SERVICE}"), get(signals))
        .route(&format!("/v1/{REPORT_SERVICE}"), get(report))
        .route(&format!("/v1/{SEC_SERVICE}"), get(sec_red_flags))
        .route("/v1/jobs/{id}", get(job_delivery))
        .with_state(state.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.port));
    println!("{}Seller {} on http://{addr} (payments via {})", state.tag, state.seller, state.contract);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Ledger signals, opened per request (SQLite connections are not shared across threads).
fn ledger(st: &AppState) -> Result<Vec<crate::store::StoredSignal>> {
    Store::open(&st.db_path)?.all()
}

async fn info(State(st): State<Arc<AppState>>) -> Response {
    let latest = ledger(&st).ok().and_then(|v| v.iter().map(|s| s.as_of).max());
    Json(json!({
        "seller": st.seller,
        "contract": st.contract,
        "network": st.near.network,
        "services": st.selling,
        "latest_signal_date": latest,
        "track_record": "/v1/track-record",
        "jobs": "/v1/jobs/<id>?receipt=<n>",
        "note": "Signals are committed on-chain by the seller account before the next open. Past signals are free at /v1/track-record; fresh ones are paid.",
    }))
    .into_response()
}

async fn track_record(State(st): State<Arc<AppState>>) -> Response {
    let now = Utc::now();
    match ledger(&st) {
        Ok(all) => {
            let entries: Vec<RecordEntry> =
                all.iter().filter(|s| entry_known_at(s.as_of, s.fill_date) <= now).map(RecordEntry::from).collect();
            Json(json!({"seller": st.seller, "committer": st.seller, "contract": st.contract, "entries": entries})).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// Reserves one redemption of `receipt`: once, or once per member if it settled a pool.
async fn reserve(st: &AppState, receipt: &Value) -> Option<Response> {
    let id = receipt["id"].as_u64().unwrap_or(u64::MAX);
    let mut allowed = 1u32;
    if receipt["memo"].as_str().is_some_and(|m| m.starts_with("pool #")) {
        match st.near.view(&st.contract, "get_pool_of_receipt", json!({"receipt_id": id})).await {
            Ok(p) if !p.is_null() => allowed = p["shares"].as_u64().unwrap_or(1) as u32,
            Ok(_) => {}
            Err(e) => return Some(err(StatusCode::BAD_GATEWAY, format!("{e:#}"))),
        }
    }
    let mut used = st.redeemed.lock().unwrap();
    let n = used.entry(id).or_insert(0);
    if *n >= allowed {
        let what = if allowed == 1 { String::new() } else { format!(" {allowed} times (once per pool member)") };
        return Some(err(StatusCode::CONFLICT, format!("receipt #{id} already redeemed{what}")));
    }
    *n += 1;
    None
}

async fn fetch_receipt(st: &AppState, q: &HashMap<String, String>) -> Result<Value, Response> {
    let id = q.get("receipt").and_then(|s| s.parse::<u64>().ok()).ok_or_else(|| err(StatusCode::BAD_REQUEST, "receipt must be a number"))?;
    match st.near.view(&st.contract, "get_receipt", json!({"id": id})).await {
        Ok(r) if r.is_null() => Err(err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{id} not found"))),
        Ok(r) => Ok(r),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, format!("{e:#}"))),
    }
}

/// Checks payment for `service` + `params`. Ok(None): paid and reserved; Ok(Some(resp)): send this instead.
async fn gate(st: &AppState, service: &str, params: &[&str], q: &HashMap<String, String>) -> Option<Response> {
    if !st.selling.contains_key(service) {
        return Some(err(StatusCode::NOT_FOUND, format!("{} does not sell {service}", st.seller)));
    }
    let request_hash = sha256_hex(&request_line(service, params));
    if !q.contains_key("receipt") {
        let price = match st.near.view(&st.contract, "get_service", json!({"service_id": service})).await {
            Ok(svc) if !svc.is_null() => svc["price"].clone(),
            Ok(_) => return Some(err(StatusCode::SERVICE_UNAVAILABLE, "service not registered")),
            Err(e) => return Some(err(StatusCode::BAD_GATEWAY, format!("{e:#}"))),
        };
        return Some(
            (
                StatusCode::PAYMENT_REQUIRED,
                Json(json!({
                    "error": "payment required",
                    "network": st.near.network,
                    "contract": st.contract,
                    "method": "pay",
                    "pool_method": "open_pool / join_pool",
                    "service_id": service,
                    "price": price,
                    "seller": st.seller,
                    "request": request_line(service, params),
                    "request_hash": request_hash,
                    "track_record": "/v1/track-record",
                })),
            )
                .into_response(),
        );
    }
    let receipt = match fetch_receipt(st, q).await {
        Ok(r) => r,
        Err(resp) => return Some(resp),
    };
    if let Err(reason) = check_receipt(&receipt, service, &st.seller, &request_hash) {
        return Some(err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{} rejected: {reason}", receipt["id"])));
    }
    reserve(st, &receipt).await
}

/// Releases a reserved redemption after a failure on our side, so the buyer can retry what it paid for.
fn release(st: &AppState, q: &HashMap<String, String>) {
    if let Some(id) = q.get("receipt").and_then(|s| s.parse::<u64>().ok()) {
        if let Some(n) = st.redeemed.lock().unwrap().get_mut(&id) {
            *n = n.saturating_sub(1);
        }
    }
}

fn parse_date(s: Option<&str>, what: &str) -> Result<NaiveDate, Response> {
    s.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("{what} must be YYYY-MM-DD")))
}

fn parse_symbol(s: Option<&str>) -> Result<String, Response> {
    let s = s.map(|s| s.trim().to_uppercase()).unwrap_or_default();
    if s.is_empty() || s.len() > 10 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "bad symbol"));
    }
    Ok(s)
}

// ---------- products (shared by paid calls and jobs) ----------

/// One session's signals, or None if there are none (never charge for an empty answer).
fn signals_for(st: &AppState, date: NaiveDate) -> Result<Option<Value>, Response> {
    let day: Vec<_> = ledger(st)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?
        .into_iter()
        .filter(|s| s.as_of == date)
        .collect();
    if day.is_empty() {
        return Ok(None);
    }
    let out: Vec<Value> = day
        .iter()
        .map(|s| {
            json!({
                "symbol": s.symbol, "setup": s.setup, "as_of": s.as_of,
                "entry": s.entry, "stop": s.stop, "target": s.target, "max_hold": s.max_hold,
                "risk_reward": (s.target - s.entry) / (s.entry - s.stop),
                "commitment_hash": RecordEntry::from(s).hash(),
            })
        })
        .collect();
    Ok(Some(json!({"date": date, "seller": st.seller, "signals": out})))
}

fn report_paths(st: &AppState, symbol: &str, date: NaiveDate) -> Result<Vec<(String, String)>, Response> {
    Store::open(&st.db_path)
        .and_then(|s| s.report_paths(symbol, date))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))
}

fn read_reports(st: &AppState, symbol: &str, date: NaiveDate, paths: Vec<(String, String)>) -> Result<Value, Response> {
    let mut reports = Vec::new();
    for (setup, path) in paths {
        let md = std::fs::read_to_string(&path).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("reading report: {e}")))?;
        reports.push(json!({"setup": setup, "markdown": md}));
    }
    Ok(json!({"symbol": symbol, "date": date, "seller": st.seller, "reports": reports}))
}

async fn filings(st: &AppState, symbol: &str, since: NaiveDate) -> Result<Value, Response> {
    let sec = st.sec.as_ref().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "SEC client not configured"))?;
    let f = sec.recent_filings(symbol, since).await.map_err(|e| err(StatusCode::BAD_GATEWAY, format!("SEC fetch failed: {e:#}")))?;
    Ok(serde_json::to_value(f).unwrap_or(Value::Null))
}

fn check_since(since: NaiveDate) -> Result<(), Response> {
    if since < Utc::now().date_naive() - TimeDelta::days(366) {
        return Err(err(StatusCode::BAD_REQUEST, "since is more than a year ago"));
    }
    Ok(())
}

// ---------- paid calls ----------

async fn signals(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let date = match parse_date(q.get("date").map(String::as_str), "date") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let body = match signals_for(&st, date) {
        Ok(Some(b)) => b,
        Ok(None) => return err(StatusCode::NOT_FOUND, format!("no signals for {date}")),
        Err(r) => return r,
    };
    let date_s = date.to_string();
    if let Some(resp) = gate(&st, SIGNALS_SERVICE, &[&date_s], &q).await {
        return resp;
    }
    eprintln!("{}sold {} signal(s) for {date} (receipt #{})", st.tag, body["signals"].as_array().map_or(0, Vec::len), q["receipt"]);
    Json(body).into_response()
}

async fn report(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let (symbol, date) = match (parse_symbol(q.get("symbol").map(String::as_str)), parse_date(q.get("date").map(String::as_str), "date")) {
        (Ok(s), Ok(d)) => (s, d),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let paths = match report_paths(&st, &symbol, date) {
        Ok(p) if p.is_empty() => return err(StatusCode::NOT_FOUND, format!("no report for {symbol} on {date}")),
        Ok(p) => p,
        Err(r) => return r,
    };
    let date_s = date.to_string();
    if let Some(resp) = gate(&st, REPORT_SERVICE, &[&symbol, &date_s], &q).await {
        return resp;
    }
    match read_reports(&st, &symbol, date, paths) {
        Ok(body) => {
            eprintln!("{}sold {symbol} {date} report (receipt #{})", st.tag, q["receipt"]);
            Json(body).into_response()
        }
        Err(r) => {
            release(&st, &q);
            r
        }
    }
}

async fn sec_red_flags(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let (symbol, since) = match (parse_symbol(q.get("symbol").map(String::as_str)), parse_date(q.get("since").map(String::as_str), "since")) {
        (Ok(s), Ok(d)) => (s, d),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    if let Err(r) = check_since(since) {
        return r;
    }
    let since_s = since.to_string();
    if let Some(resp) = gate(&st, SEC_SERVICE, &[&symbol, &since_s], &q).await {
        return resp;
    }
    match filings(&st, &symbol, since).await {
        Ok(body) => {
            eprintln!("{}sold {symbol} filings since {since}: {} filing(s) (receipt #{})", st.tag, body.as_array().map_or(0, Vec::len), q["receipt"]);
            Json(body).into_response()
        }
        Err(r) => {
            release(&st, &q);
            r
        }
    }
}

// ---------- jobs ----------

/// What a job topic asks for. Topics are `kind|param|param`, e.g. `sec-red-flags|JPM|2026-10-10`.
#[derive(Debug, PartialEq)]
pub enum JobTopic {
    Filings { symbol: String, since: NaiveDate },
    Signals { date: NaiveDate },
    Report { symbol: String, date: NaiveDate },
}

impl JobTopic {
    pub fn parse(topic: &str) -> Option<Self> {
        let p: Vec<&str> = topic.split('|').collect();
        let date = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        let sym = |s: &str| parse_symbol(Some(s)).ok();
        match p.as_slice() {
            [k, s, d] if *k == SEC_SERVICE => Some(Self::Filings { symbol: sym(s)?, since: date(d)? }),
            [k, d] if *k == SIGNALS_SERVICE => Some(Self::Signals { date: date(d)? }),
            [k, s, d] if *k == REPORT_SERVICE => Some(Self::Report { symbol: sym(s)?, date: date(d)? }),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Filings { .. } => SEC_SERVICE,
            Self::Signals { .. } => SIGNALS_SERVICE,
            Self::Report { .. } => REPORT_SERVICE,
        }
    }
}

/// Whether this seller can deliver `topic` right now (checked before bidding, so it never wins what it cannot ship).
fn can_deliver(st: &AppState, t: &JobTopic) -> bool {
    match t {
        JobTopic::Filings { since, .. } => st.sec.is_some() && check_since(*since).is_ok(),
        JobTopic::Signals { date } => matches!(signals_for(st, *date), Ok(Some(_))),
        JobTopic::Report { symbol, date } => matches!(report_paths(st, symbol, *date), Ok(p) if !p.is_empty()),
    }
}

async fn produce(st: &AppState, t: &JobTopic) -> Result<Value, Response> {
    match t {
        JobTopic::Filings { symbol, since } => filings(st, symbol, *since).await,
        JobTopic::Signals { date } => signals_for(st, *date)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "no signals")),
        JobTopic::Report { symbol, date } => {
            let paths = report_paths(st, symbol, *date)?;
            read_reports(st, symbol, *date, paths)
        }
    }
}

async fn job_delivery(State(st): State<Arc<AppState>>, Path(id): Path<u64>, Query(q): Q) -> Response {
    let job = match st.near.view(&st.contract, "get_job", json!({"id": id})).await {
        Ok(j) if j.is_null() => return err(StatusCode::NOT_FOUND, format!("no job #{id}")),
        Ok(j) => j,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    };
    // Views read final state, a couple of blocks behind the award the buyer just made: ask it to retry.
    if job["winner"].is_null() && job["status"].as_str() == Some("Open") {
        return err(StatusCode::SERVICE_UNAVAILABLE, format!("job #{id} award not final yet; retry"));
    }
    if job["winner"].as_str() != Some(st.seller.as_str()) {
        return err(StatusCode::PAYMENT_REQUIRED, format!("job #{id} was not awarded to {}", st.seller));
    }
    let receipt = match fetch_receipt(&st, &q).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if job["receipt"].as_u64() != receipt["id"].as_u64() {
        return err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{} is not job #{id}'s payment", receipt["id"]));
    }
    let topic = job["topic"].as_str().unwrap_or_default();
    if let Err(reason) = check_receipt(&receipt, &format!("job:{id}"), &st.seller, &job_request_hash(id, topic)) {
        return err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{} rejected: {reason}", receipt["id"]));
    }
    let Some(t) = JobTopic::parse(topic) else {
        return err(StatusCode::UNPROCESSABLE_ENTITY, format!("cannot parse topic {topic:?}"));
    };
    if let Some(resp) = reserve(&st, &receipt).await {
        return resp;
    }
    match produce(&st, &t).await {
        Ok(body) => {
            eprintln!("{}delivered job #{id} ({topic}) against receipt #{}", st.tag, receipt["id"]);
            Json(json!({"job": id, "topic": topic, "seller": st.seller, "result": body})).into_response()
        }
        Err(r) => {
            release(&st, &q);
            r
        }
    }
}

/// Watches the job board and bids on open jobs this seller can deliver at its listed price.
async fn bid_loop(st: Arc<AppState>, signer: Signer, policy: BidPolicy) {
    let full_access = st.near.is_full_access(&signer).await.unwrap_or(false);
    let mut bid_on: HashSet<u64> = HashSet::new();
    loop {
        if let Err(e) = bid_round(&st, &signer, &policy, full_access, &mut bid_on).await {
            eprintln!("{}bidding: {e:#}", st.tag);
        }
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    }
}

async fn bid_round(st: &AppState, signer: &Signer, policy: &BidPolicy, full_access: bool, bid_on: &mut HashSet<u64>) -> Result<()> {
    let n = st.near.view(&st.contract, "job_count", json!({})).await?.as_u64().unwrap_or(0);
    let from = n.saturating_sub(20);
    let jobs = st.near.view(&st.contract, "get_jobs", json!({"from": from, "limit": 20})).await?;
    let now_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
    for j in jobs.as_array().into_iter().flatten() {
        let id = j["id"].as_u64().unwrap_or(0);
        if bid_on.contains(&id) || j["status"].as_str() != Some("Open") || j["deadline_ns"].as_u64().unwrap_or(0) <= now_ns {
            continue;
        }
        bid_on.insert(id);
        let topic = j["topic"].as_str().unwrap_or_default();
        let Some(t) = JobTopic::parse(topic) else { continue };
        let Some(&price) = policy.prices.get(t.kind()) else { continue };
        if j["buyer"].as_str() == Some(st.seller.as_str()) || j["owner"].as_str() == Some(st.seller.as_str()) {
            continue;
        }
        if price > u128_field(j, "max_price")? {
            eprintln!("{}job #{id} ({topic}): max price {} is below our {}; not bidding", st.tag, fmt_near(u128_field(j, "max_price")?), fmt_near(price));
            continue;
        }
        if !can_deliver(st, &t) {
            eprintln!("{}job #{id} ({topic}): cannot deliver; not bidding", st.tag);
            continue;
        }
        let deposit = if full_access { super::client::parse_near("0.01")? } else { 0 };
        let r = st
            .near
            .call(signer, &st.contract, "bid", json!({"job_id": id, "price": price.to_string(), "endpoint": policy.endpoint}), deposit)
            .await?;
        eprintln!("{}bid {} on job #{id} ({topic}): {}", st.tag, fmt_near(price), st.near.explorer_tx(&r.hash));
    }
    Ok(())
}

fn check_receipt(r: &Value, service: &str, seller: &str, request_hash: &str) -> std::result::Result<(), String> {
    if r["service_id"].as_str() != Some(service) {
        return Err(format!("for service {}", r["service_id"]));
    }
    if r["provider"].as_str() != Some(seller) {
        return Err(format!("paid to {}", r["provider"]));
    }
    if r["request_hash"].as_str() != Some(request_hash) {
        return Err("paid for a different request".into());
    }
    if u128_field(r, "amount").map_err(|e| e.to_string())? == 0 {
        return Err("zero amount".into());
    }
    let ts = r["timestamp_ns"].as_u64().unwrap_or(0) / 1_000_000_000;
    if (Utc::now().timestamp() as u64).saturating_sub(ts) > MAX_RECEIPT_AGE_SECS {
        return Err("expired".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(hash: &str, age_secs: u64) -> Value {
        let ts = (Utc::now().timestamp() as u64 - age_secs) * 1_000_000_000;
        json!({"service_id": SIGNALS_SERVICE, "provider": "axiom.testnet", "request_hash": hash, "amount": "10", "timestamp_ns": ts})
    }

    #[test]
    fn receipt_checks() {
        let ok = |r: &Value, svc, seller, h| check_receipt(r, svc, seller, h);
        assert!(ok(&receipt("h", 5), SIGNALS_SERVICE, "axiom.testnet", "h").is_ok());
        assert!(ok(&receipt("h", 5), REPORT_SERVICE, "axiom.testnet", "h").unwrap_err().contains("for service"));
        assert!(ok(&receipt("h", 5), SIGNALS_SERVICE, "other.testnet", "h").unwrap_err().contains("paid to"));
        assert!(ok(&receipt("h", 5), SIGNALS_SERVICE, "axiom.testnet", "x").unwrap_err().contains("different request"));
        assert!(ok(&receipt("h", 3600), SIGNALS_SERVICE, "axiom.testnet", "h").unwrap_err().contains("expired"));
    }

    #[test]
    fn job_topics_parse() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        assert_eq!(JobTopic::parse("sec-red-flags|jpm|2026-10-10"), Some(JobTopic::Filings { symbol: "JPM".into(), since: d }));
        assert_eq!(JobTopic::parse("axiom-signals|2026-10-10"), Some(JobTopic::Signals { date: d }));
        assert_eq!(JobTopic::parse("axiom-report|XOM|2026-10-10").unwrap().kind(), REPORT_SERVICE);
        assert_eq!(JobTopic::parse("axiom-report|XOM"), None);
        assert_eq!(JobTopic::parse("sec-red-flags|J PM|2026-10-10"), None);
        assert_eq!(JobTopic::parse("weather|NYC|2026-10-10"), None);
    }
}
