//! A paid evidence provider: another agent that sells SEC filings (with 8-K red-flag codes) per call.
//! It serves a request only against an Axiom Pay receipt that names this provider, this service and the
//! hash of this exact request, is recent, and has not been redeemed before. Checking costs one free view call.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{NaiveDate, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::client::{sha256_hex, u128_field, Near};
use super::{request_line, SEC_SERVICE};
use crate::data::sec::Sec;

/// A receipt older than this cannot be redeemed.
const MAX_RECEIPT_AGE_SECS: u64 = 15 * 60;

struct AppState {
    near: Near,
    contract: String,
    provider: String,
    sec: Sec,
    redeemed: Mutex<HashSet<u64>>,
}

#[derive(Deserialize)]
struct Params {
    symbol: String,
    since: String,
    receipt: Option<u64>,
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

pub async fn serve(port: u16, provider: String, contract: String, sec_user_agent: &str) -> Result<()> {
    let state = Arc::new(AppState {
        near: Near::from_env(),
        contract,
        provider,
        sec: Sec::new(sec_user_agent).await.context("loading SEC ticker map")?,
        redeemed: Mutex::new(HashSet::new()),
    });
    let svc = state.near.view(&state.contract, "get_service", json!({"service_id": SEC_SERVICE})).await?;
    if svc.is_null() {
        eprintln!("warning: {SEC_SERVICE} is not registered on {} yet (run `axiom near register`)", state.contract);
    } else if svc["provider"].as_str() != Some(state.provider.as_str()) {
        anyhow::bail!("{SEC_SERVICE} on {} belongs to {}, not {}", state.contract, svc["provider"], state.provider);
    }
    let app = Router::new()
        .route("/", get(info))
        .route(&format!("/v1/{SEC_SERVICE}"), get(sec_red_flags))
        .with_state(state.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("Provider {} selling {SEC_SERVICE} on http://{addr} (payments via {})", state.provider, state.contract);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn info(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"provider": st.provider, "contract": st.contract, "services": [SEC_SERVICE], "network": st.near.network}))
}

async fn sec_red_flags(State(st): State<Arc<AppState>>, Query(p): Query<Params>) -> Response {
    let symbol = p.symbol.trim().to_uppercase();
    if symbol.is_empty() || symbol.len() > 10 || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return err(StatusCode::BAD_REQUEST, "bad symbol");
    }
    let Ok(since) = NaiveDate::parse_from_str(&p.since, "%Y-%m-%d") else {
        return err(StatusCode::BAD_REQUEST, "since must be YYYY-MM-DD");
    };
    if since < Utc::now().date_naive() - TimeDelta::days(366) {
        return err(StatusCode::BAD_REQUEST, "since is more than a year ago");
    }
    let request_hash = sha256_hex(&request_line(SEC_SERVICE, &symbol, since));

    let Some(id) = p.receipt else {
        // 402: quote the terms. The buyer recomputes the hash itself and checks the price on-chain.
        let price = match st.near.view(&st.contract, "get_service", json!({"service_id": SEC_SERVICE})).await {
            Ok(svc) if !svc.is_null() => svc["price"].clone(),
            Ok(_) => return err(StatusCode::SERVICE_UNAVAILABLE, "service not registered"),
            Err(e) => return err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
        };
        return (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "error": "payment required",
                "network": st.near.network,
                "contract": st.contract,
                "method": "pay",
                "service_id": SEC_SERVICE,
                "price": price,
                "request": request_line(SEC_SERVICE, &symbol, since),
                "request_hash": request_hash,
            })),
        )
            .into_response();
    };

    let receipt = match st.near.view(&st.contract, "get_receipt", json!({"id": id})).await {
        Ok(r) if r.is_null() => return err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{id} not found")),
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    };
    if let Err(reason) = check_receipt(&receipt, &st.provider, &request_hash) {
        return err(StatusCode::PAYMENT_REQUIRED, format!("receipt #{id} rejected: {reason}"));
    }
    if !st.redeemed.lock().unwrap().insert(id) {
        return err(StatusCode::CONFLICT, format!("receipt #{id} already redeemed"));
    }
    match st.sec.recent_filings(&symbol, since).await {
        Ok(filings) => {
            eprintln!("served {symbol} since {since}: {} filing(s) for receipt #{id}", filings.len());
            Json(filings).into_response()
        }
        Err(e) => {
            st.redeemed.lock().unwrap().remove(&id); // let the buyer retry what it paid for
            err(StatusCode::BAD_GATEWAY, format!("SEC fetch failed: {e:#}"))
        }
    }
}

fn check_receipt(r: &Value, provider: &str, request_hash: &str) -> std::result::Result<(), String> {
    if r["service_id"].as_str() != Some(SEC_SERVICE) {
        return Err(format!("for service {}", r["service_id"]));
    }
    if r["provider"].as_str() != Some(provider) {
        return Err(format!("paid to {}", r["provider"]));
    }
    if r["request_hash"].as_str() != Some(request_hash) {
        return Err("paid for a different request".into());
    }
    if u128_field(r, "amount").map_err(|e| e.to_string())? == 0 {
        return Err("zero amount".into());
    }
    let ts = r["timestamp_ns"].as_u64().unwrap_or(0) / 1_000_000_000;
    let now = Utc::now().timestamp() as u64;
    if now.saturating_sub(ts) > MAX_RECEIPT_AGE_SECS {
        return Err("expired".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(hash: &str, age_secs: u64) -> Value {
        let ts = (Utc::now().timestamp() as u64 - age_secs) * 1_000_000_000;
        json!({"service_id": SEC_SERVICE, "provider": "sec.testnet", "request_hash": hash, "amount": "10", "timestamp_ns": ts})
    }

    #[test]
    fn receipt_checks() {
        assert!(check_receipt(&receipt("h", 5), "sec.testnet", "h").is_ok());
        assert!(check_receipt(&receipt("h", 5), "other.testnet", "h").unwrap_err().contains("paid to"));
        assert!(check_receipt(&receipt("h", 5), "sec.testnet", "x").unwrap_err().contains("different request"));
        assert!(check_receipt(&receipt("h", 3600), "sec.testnet", "h").unwrap_err().contains("expired"));
    }
}
