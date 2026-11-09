//! Alpaca Market Data API v2: daily bars and news (Benzinga feed).
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::config::Config;
use crate::models::{Bar, NewsItem};

const BARS_URL: &str = "https://data.alpaca.markets/v2/stocks/bars";
const NEWS_URL: &str = "https://data.alpaca.markets/v1beta1/news";

pub struct Alpaca {
    http: reqwest::Client,
    key: String,
    secret: String,
    feed: String,
}

#[derive(Deserialize)]
struct BarsResp {
    #[serde(default)]
    bars: Option<HashMap<String, Vec<RawBar>>>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct RawBar {
    t: DateTime<Utc>,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
}

#[derive(Deserialize)]
struct NewsResp {
    #[serde(default)]
    news: Vec<RawNews>,
}

#[derive(Deserialize)]
struct RawNews {
    headline: String,
    #[serde(default)]
    summary: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    source: String,
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl Alpaca {
    pub fn new(cfg: &Config) -> Self {
        Self {
            // Generous total timeout for slow connections; fail fast only if we can't connect at all.
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(300))
                .build()
                .expect("http client"),
            key: cfg.alpaca_key.clone(),
            secret: cfg.alpaca_secret.clone(),
            feed: cfg.alpaca_feed.clone(),
        }
    }

    /// GET with retries on rate limits, server errors, and network timeouts/drops.
    async fn get<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)]) -> Result<T> {
        const ATTEMPTS: u64 = 5;
        let mut last_err = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                let wait = 2 + attempt * 3;
                eprintln!("    retrying in {wait}s (attempt {}/{ATTEMPTS}): {last_err}", attempt + 1);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            let resp = match self
                .http
                .get(url)
                .header("APCA-API-KEY-ID", &self.key)
                .header("APCA-API-SECRET-KEY", &self.secret)
                .query(query)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("network error: {e}");
                    continue;
                }
            };
            let status = resp.status();
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = format!("HTTP {status}");
                continue;
            }
            if !status.is_success() {
                bail!("Alpaca {status}: {}", resp.text().await.unwrap_or_default());
            }
            // Read the whole body first so a slow/dropped download is retried rather than fatal.
            let body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    last_err = format!("download interrupted: {e}");
                    continue;
                }
            };
            return serde_json::from_slice(&body).context("Alpaca returned unexpected JSON");
        }
        bail!("Alpaca: gave up after {ATTEMPTS} attempts ({last_err}). Check your internet connection.")
    }

    /// Adjusted daily bars for many symbols, paginated. Returned series are sorted by date.
    pub async fn daily_bars(
        &self,
        symbols: &[String],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        self.bars(symbols, "1Day", "all", start, end).await
    }

    /// Monthly bars as originally quoted (no split/dividend adjustment), for price ratios
    /// against per-share numbers as they were reported at the time.
    pub async fn monthly_raw_bars(
        &self,
        symbols: &[String],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        self.bars(symbols, "1Month", "raw", start, end).await
    }

    async fn bars(
        &self,
        symbols: &[String],
        timeframe: &str,
        adjustment: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        let mut out: HashMap<String, Vec<Bar>> = HashMap::new();
        let (mut pages, mut total) = (0usize, 0usize);
        for chunk in symbols.chunks(100) {
            let syms = chunk.join(",");
            let mut page: Option<String> = None;
            loop {
                let mut q: Vec<(&str, String)> = vec![
                    ("symbols", syms.clone()),
                    ("timeframe", timeframe.into()),
                    ("start", ts(start)),
                    ("end", ts(end)),
                    ("adjustment", adjustment.into()),
                    ("feed", self.feed.clone()),
                    // Small pages (~200KB) so each request finishes quickly even on a slow connection.
                    ("limit", "2000".into()),
                ];
                if let Some(p) = &page {
                    q.push(("page_token", p.clone()));
                }
                let resp: BarsResp = self.get(BARS_URL, &q).await?;
                let bars = resp.bars.unwrap_or_default();
                pages += 1;
                total += bars.values().map(Vec::len).sum::<usize>();
                eprint!("\r  downloaded page {pages} ({total} bars)   ");
                for (sym, raws) in bars {
                    out.entry(sym).or_default().extend(raws.into_iter().map(|r| Bar {
                        date: r.t.date_naive(),
                        open: r.o,
                        high: r.h,
                        low: r.l,
                        close: r.c,
                        volume: r.v,
                    }));
                }
                match resp.next_page_token {
                    Some(t) if !t.is_empty() => page = Some(t),
                    _ => break,
                }
            }
        }
        eprintln!();
        for series in out.values_mut() {
            series.sort_by_key(|b| b.date);
            series.dedup_by_key(|b| b.date);
        }
        Ok(out)
    }

    /// Most recent news for a symbol since `since`, newest first.
    pub async fn news(&self, symbol: &str, since: DateTime<Utc>, limit: usize) -> Result<Vec<NewsItem>> {
        let q = vec![
            ("symbols", symbol.to_string()),
            ("start", ts(since)),
            ("limit", limit.to_string()),
            ("sort", "desc".into()),
        ];
        let resp: NewsResp = self.get(NEWS_URL, &q).await?;
        Ok(resp
            .news
            .into_iter()
            .map(|n| NewsItem {
                headline: n.headline,
                summary: n.summary,
                published_at: n.created_at,
                url: n.url,
                source: n.source,
            })
            .collect())
    }
}
