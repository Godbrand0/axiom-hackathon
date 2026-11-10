//! Seller server: sells services per call against Axiom Pay receipts.
//!
//! It serves whichever of these services the seller account has registered on-chain:
//! * `axiom-signals` / `axiom-report`: Axiom's own fresh research (the business).
//! * `sec-red-flags`: SEC filings with 8-K red-flag codes (an evidence seller Axiom itself buys from).
//!
//! `/v1/track-record` is free: every signal whose entry is already known, with the fields needed to recompute
//! its hash, so a buyer can check the seller's on-chain commitments before paying for fresh ones.
//! A paid request is served only against a receipt that names this seller, this service and the hash of this
//! exact request, is under 15 minutes old, and has not been redeemed before.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{NaiveDate, TimeDelta, Utc};
use serde_json::{json, Value};

use super::client::{fmt_near, sha256_hex, u128_field, Near};
use super::{entry_known_at, request_line, RecordEntry, REPORT_SERVICE, SEC_SERVICE, SIGNALS_SERVICE};
use crate::data::sec::Sec;
use crate::store::Store;

const MAX_RECEIPT_AGE_SECS: u64 = 15 * 60;

struct AppState {
    near: Near,
    contract: String,
    seller: String,
    db_path: String,
    sec: Option<Sec>,
    /// service id -> description, for the services this seller registered.
    selling: HashMap<String, String>,
    redeemed: Mutex<HashSet<u64>>,
}

type Q = Query<HashMap<String, String>>;

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

pub async fn serve(port: u16, seller: String, contract: String, db_path: String, sec_user_agent: Option<String>) -> Result<()> {
    let near = Near::from_env();
    let services = near.view(&contract, "list_services", json!({})).await?;
    let mut selling = HashMap::new();
    for pair in services.as_array().into_iter().flatten() {
        if pair[1]["provider"].as_str() == Some(seller.as_str()) {
            let id = pair[0].as_str().unwrap_or_default().to_string();
            println!("  selling {id:<16} {}/call", fmt_near(u128_field(&pair[1], "price")?));
            selling.insert(id, pair[1]["description"].as_str().unwrap_or_default().to_string());
        }
    }
    if selling.is_empty() {
        anyhow::bail!("{seller} has no services on {contract}; run `axiom near register --service <id> --price <NEAR> --as {seller}`");
    }
    let sec = match (selling.contains_key(SEC_SERVICE), sec_user_agent) {
        (true, Some(ua)) => Some(Sec::new(&ua).await.context("loading SEC ticker map")?),
        (true, None) => anyhow::bail!("selling {SEC_SERVICE} needs SEC_USER_AGENT"),
        _ => None,
    };
    let state = Arc::new(AppState { near, contract, seller, db_path, sec, selling, redeemed: Mutex::new(HashSet::new()) });
    let app = Router::new()
        .route("/", get(info))
        .route("/v1/track-record", get(track_record))
        .route(&format!("/v1/{SIGNALS_SERVICE}"), get(signals))
        .route(&format!("/v1/{REPORT_SERVICE}"), get(report))
        .route(&format!("/v1/{SEC_SERVICE}"), get(sec_red_flags))
        .with_state(state.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("Seller {} on http://{addr} (payments via {})", state.seller, state.contract);
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

/// Checks payment for `service` + `params`. Ok(None): paid and reserved; Ok(Some(resp)): send this instead.
async fn gate(st: &AppState, service: &str, params: &[&str], q: &HashMap<String, String>) -> Option<Response> {
    if !st.selling.contains_key(service) {
        return Some(err(StatusCode::NOT_FOUND, format!("{} does not sell {service}", st.seller)));
    }
    let request_hash = sha256_hex(&request_line(service, params));
    let Some(id) = q.get("receipt") else {
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
    };
    let Ok(id) = id.parse::<u64>() else {
        return Some(err(StatusCode::BAD_REQUEST, "receipt must be a number"));
    };
    let receipt = match st.near.view(&st.contract, "get_receipt", json!({"id": id})).await {
        Ok(r) if r.is_null() => return Some(err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{id} not found"))),
        Ok(r) => r,
        Err(e) => return Some(err(StatusCode::BAD_GATEWAY, format!("{e:#}"))),
    };
    if let Err(reason) = check_receipt(&receipt, service, &st.seller, &request_hash) {
        return Some(err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{id} rejected: {reason}")));
    }
    if !st.redeemed.lock().unwrap().insert(id) {
        return Some(err(StatusCode::CONFLICT, format!("receipt #{id} already redeemed")));
    }
    None
}

/// Releases a reserved receipt after a failure on our side, so the buyer can retry what it paid for.
fn release(st: &AppState, q: &HashMap<String, String>) {
    if let Some(id) = q.get("receipt").and_then(|s| s.parse::<u64>().ok()) {
        st.redeemed.lock().unwrap().remove(&id);
    }
}

fn param_date(q: &HashMap<String, String>) -> Result<NaiveDate, Response> {
    q.get("date")
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD"))
}

fn param_symbol(q: &HashMap<String, String>) -> Result<String, Response> {
    let s = q.get("symbol").map(|s| s.trim().to_uppercase()).unwrap_or_default();
    if s.is_empty() || s.len() > 10 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "bad symbol"));
    }
    Ok(s)
}

async fn signals(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let date = match param_date(&q) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // Never charge for an empty answer: check there is something to sell before quoting a price.
    let day: Vec<_> = match ledger(&st) {
        Ok(all) => all.into_iter().filter(|s| s.as_of == date).collect(),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    if day.is_empty() {
        return err(StatusCode::NOT_FOUND, format!("no signals for {date}"));
    }
    let date_s = date.to_string();
    if let Some(resp) = gate(&st, SIGNALS_SERVICE, &[&date_s], &q).await {
        return resp;
    }
    let out: Vec<Value> = day
        .iter()
        .map(|s| {
            let e = RecordEntry::from(s);
            json!({
                "symbol": s.symbol, "setup": s.setup, "as_of": s.as_of,
                "entry": s.entry, "stop": s.stop, "target": s.target, "max_hold": s.max_hold,
                "risk_reward": (s.target - s.entry) / (s.entry - s.stop),
                "commitment_hash": e.hash(),
            })
        })
        .collect();
    eprintln!("sold {} signal(s) for {date} (receipt #{})", out.len(), q["receipt"]);
    Json(json!({"date": date, "seller": st.seller, "signals": out})).into_response()
}

async fn report(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let (symbol, date) = match (param_symbol(&q), param_date(&q)) {
        (Ok(s), Ok(d)) => (s, d),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let paths = match Store::open(&st.db_path).and_then(|s| s.report_paths(&symbol, date)) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    if paths.is_empty() {
        return err(StatusCode::NOT_FOUND, format!("no report for {symbol} on {date}"));
    }
    let date_s = date.to_string();
    if let Some(resp) = gate(&st, REPORT_SERVICE, &[&symbol, &date_s], &q).await {
        return resp;
    }
    let mut reports = Vec::new();
    for (setup, path) in paths {
        match std::fs::read_to_string(&path) {
            Ok(md) => reports.push(json!({"setup": setup, "markdown": md})),
            Err(e) => {
                release(&st, &q);
                return err(StatusCode::INTERNAL_SERVER_ERROR, format!("reading report: {e}"));
            }
        }
    }
    eprintln!("sold {symbol} {date} report (receipt #{})", q["receipt"]);
    Json(json!({"symbol": symbol, "date": date, "seller": st.seller, "reports": reports})).into_response()
}

async fn sec_red_flags(State(st): State<Arc<AppState>>, Query(q): Q) -> Response {
    let symbol = match param_symbol(&q) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(since) = q.get("since").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) else {
        return err(StatusCode::BAD_REQUEST, "since must be YYYY-MM-DD");
    };
    if since < Utc::now().date_naive() - TimeDelta::days(366) {
        return err(StatusCode::BAD_REQUEST, "since is more than a year ago");
    }
    let since_s = since.to_string();
    if let Some(resp) = gate(&st, SEC_SERVICE, &[&symbol, &since_s], &q).await {
        return resp;
    }
    let Some(sec) = &st.sec else {
        release(&st, &q);
        return err(StatusCode::SERVICE_UNAVAILABLE, "SEC client not configured");
    };
    match sec.recent_filings(&symbol, since).await {
        Ok(filings) => {
            eprintln!("sold {symbol} filings since {since}: {} filing(s) (receipt #{})", filings.len(), q["receipt"]);
            Json(filings).into_response()
        }
        Err(e) => {
            release(&st, &q);
            err(StatusCode::BAD_GATEWAY, format!("SEC fetch failed: {e:#}"))
        }
    }
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
}
