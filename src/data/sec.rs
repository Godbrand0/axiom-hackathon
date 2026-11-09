//! SEC EDGAR: ticker→CIK map and recent filings from the submissions API.
//! Fair-access policy: identify yourself in User-Agent and stay under 10 req/s.
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::fundamentals::Fact;
use crate::models::Filing;

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";

pub struct Sec {
    http: reqwest::Client,
    ciks: HashMap<String, u64>,
}

#[derive(Deserialize)]
struct TickerRow {
    cik_str: u64,
    ticker: String,
}

#[derive(Deserialize)]
struct Submissions {
    filings: Filings,
}

#[derive(Deserialize)]
struct Filings {
    recent: Recent,
}

#[derive(Deserialize)]
struct Recent {
    #[serde(rename = "accessionNumber")]
    accession: Vec<String>,
    #[serde(rename = "filingDate")]
    filing_date: Vec<String>,
    form: Vec<String>,
    #[serde(rename = "primaryDocument")]
    primary_document: Vec<String>,
    #[serde(default)]
    items: Vec<String>,
}

/// Forms worth surfacing for a trade thesis.
const FORMS: &[&str] = &["8-K", "10-Q", "10-K", "4", "SC 13D", "SC 13G", "S-1", "S-3", "424B5"];

impl Sec {
    pub async fn new(user_agent: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(120))
            .build()?;
        let mut sec = Self { http, ciks: HashMap::new() };
        let rows: HashMap<String, TickerRow> = sec.get(TICKERS_URL).await.context("loading SEC ticker map")?;
        sec.ciks = rows.into_values().map(|r| (r.ticker.to_uppercase(), r.cik_str)).collect();
        Ok(sec)
    }

    /// GET with retries on network drops, 429 and 5xx. Ok(None) on 404 (e.g. a concept the company doesn't report).
    async fn get_bytes(&self, url: &str) -> Result<Option<Vec<u8>>> {
        const ATTEMPTS: u64 = 5;
        let mut last_err = String::new();
        for attempt in 0..ATTEMPTS {
            // Stay well under SEC's 10 requests/second, and back off on retries.
            let wait_ms = if attempt == 0 { 150 } else { 1000 * (2 + attempt * 3) };
            if attempt > 0 {
                eprintln!("    SEC retry in {}s (attempt {}/{ATTEMPTS}): {last_err}", wait_ms / 1000, attempt + 1);
            }
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            let resp = match self.http.get(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("network error: {e}");
                    continue;
                }
            };
            let status = resp.status();
            if status.as_u16() == 404 {
                return Ok(None);
            }
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = format!("HTTP {status}");
                continue;
            }
            if !status.is_success() {
                bail!("SEC {status} for {url}");
            }
            match resp.bytes().await {
                Ok(b) => return Ok(Some(b.to_vec())),
                Err(e) => last_err = format!("download interrupted: {e}"),
            }
        }
        bail!("SEC: gave up after {ATTEMPTS} attempts ({last_err})")
    }

    async fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let body = self.get_bytes(url).await?.with_context(|| format!("SEC 404 for {url}"))?;
        serde_json::from_slice(&body).context("SEC returned unexpected JSON")
    }

    pub fn has_cik(&self, symbol: &str) -> bool {
        self.cik(symbol).is_some()
    }

    /// Every 10-K/10-Q value ever reported for one us-gaap concept, with its filing date.
    /// Ok(None) if the company has no CIK or never reported the concept.
    pub async fn concept(&self, symbol: &str, concept: &str) -> Result<Option<Vec<Fact>>> {
        let Some(cik) = self.cik(symbol) else { return Ok(None) };
        let url = format!("https://data.sec.gov/api/xbrl/companyconcept/CIK{cik:010}/us-gaap/{concept}.json");
        let Some(body) = self.get_bytes(&url).await? else { return Ok(None) };
        let v: serde_json::Value = serde_json::from_slice(&body).context("SEC concept JSON")?;
        Ok(Some(parse_concept(&v)))
    }

    /// Alpaca uses "BRK.B"; SEC uses "BRK-B".
    fn cik(&self, symbol: &str) -> Option<u64> {
        self.ciks.get(&symbol.to_uppercase().replace('.', "-")).copied()
    }

    pub async fn recent_filings(&self, symbol: &str, since: NaiveDate) -> Result<Vec<Filing>> {
        let Some(cik) = self.cik(symbol) else { return Ok(vec![]) };
        let url = format!("https://data.sec.gov/submissions/CIK{cik:010}.json");
        let subs: Submissions = self.get(&url).await?;
        let r = subs.filings.recent;
        let mut out = Vec::new();
        for i in 0..r.accession.len() {
            let Ok(filed) = NaiveDate::parse_from_str(&r.filing_date[i], "%Y-%m-%d") else { continue };
            if filed < since {
                break; // "recent" is newest-first
            }
            if !FORMS.contains(&r.form[i].as_str()) {
                continue;
            }
            let acc = r.accession[i].replace('-', "");
            out.push(Filing {
                form: r.form[i].clone(),
                filed,
                items: r.items.get(i).cloned().unwrap_or_default(),
                url: format!(
                    "https://www.sec.gov/Archives/edgar/data/{cik}/{acc}/{}",
                    r.primary_document.get(i).map(String::as_str).unwrap_or("")
                ),
            });
        }
        Ok(out)
    }
}

fn parse_concept(v: &serde_json::Value) -> Vec<Fact> {
    let date = |x: &serde_json::Value| x.as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let mut out = Vec::new();
    let Some(units) = v.get("units").and_then(|u| u.as_object()) else { return out };
    for rows in units.values().filter_map(|r| r.as_array()) {
        for r in rows {
            let form = r.get("form").and_then(|f| f.as_str()).unwrap_or("");
            if !(form.starts_with("10-K") || form.starts_with("10-Q")) {
                continue;
            }
            let (Some(end), Some(filed), Some(val)) =
                (r.get("end").and_then(date), r.get("filed").and_then(date), r.get("val").and_then(|x| x.as_f64()))
            else {
                continue;
            };
            out.push(Fact { start: r.get("start").and_then(date), end, val, filed });
        }
    }
    out
}

/// Human-readable meaning of 8-K item codes.
pub fn item_description(code: &str) -> &'static str {
    match code.trim() {
        "1.01" => "entry into a material agreement",
        "1.02" => "termination of a material agreement",
        "1.03" => "bankruptcy or receivership",
        "1.05" => "material cybersecurity incident",
        "2.01" => "completed acquisition or disposition",
        "2.02" => "results of operations (earnings release)",
        "2.03" => "new material debt obligation",
        "2.04" => "triggering event accelerating debt",
        "2.05" => "exit/restructuring costs",
        "2.06" => "material impairment",
        "3.01" => "delisting notice / listing-standard failure",
        "3.02" => "unregistered equity sale",
        "4.01" => "change of auditor",
        "4.02" => "prior financials no longer reliable (restatement)",
        "5.02" => "director/officer departure or appointment",
        "5.07" => "shareholder vote results",
        "7.01" => "Reg FD disclosure",
        "8.01" => "other material events",
        "9.01" => "financial statements and exhibits",
        _ => "other item",
    }
}

/// 8-K items that are red flags for a long thesis.
pub fn is_red_flag(code: &str) -> bool {
    matches!(code.trim(), "1.03" | "1.05" | "2.04" | "2.06" | "3.01" | "4.01" | "4.02")
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_concept_json_keeping_only_periodic_reports() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"units":{"USD/shares":[
                {"start":"2024-01-01","end":"2024-03-31","val":1.5,"form":"10-Q","filed":"2024-05-01"},
                {"end":"2024-03-31","val":9.0,"form":"8-K","filed":"2024-04-20"},
                {"end":"2024-12-31","val":100,"form":"10-K","filed":"2025-02-10"}]}}"#,
        )
        .unwrap();
        let f = super::parse_concept(&v);
        assert_eq!(f.len(), 2);
        assert!(f[0].start.is_some() && f[1].start.is_none());
        assert_eq!(f[1].val, 100.0);
    }
}
