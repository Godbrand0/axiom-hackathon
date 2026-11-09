use anyhow::{Context, Result};

/// Runtime configuration, read from environment (and `.env` via dotenvy).
pub struct Config {
    pub alpaca_key: String,
    pub alpaca_secret: String,
    pub alpaca_feed: String,
    pub sec_user_agent: String,
    /// OpenAI-compatible base URL (Ollama by default).
    pub llm_base_url: String,
    pub llm_api_key: Option<String>,
    /// LLM notes are enabled only when a model is configured.
    pub llm_model: Option<String>,
    pub telegram: Option<(String, String)>,
    pub db_path: String,
    pub reports_dir: String,
}

fn required(key: &str) -> Result<String> {
    optional(key).with_context(|| format!("missing env var {key} (copy .env.example to .env)"))
}

fn optional(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let telegram = match (optional("TELEGRAM_BOT_TOKEN"), optional("TELEGRAM_CHAT_ID")) {
            (Some(t), Some(c)) => Some((t, c)),
            _ => None,
        };
        Ok(Self {
            alpaca_key: required("ALPACA_API_KEY_ID")?,
            alpaca_secret: required("ALPACA_API_SECRET_KEY")?,
            alpaca_feed: optional("ALPACA_DATA_FEED").unwrap_or_else(|| "sip".into()),
            sec_user_agent: required("SEC_USER_AGENT")?,
            llm_base_url: optional("AXIOM_LLM_BASE_URL").unwrap_or_else(|| "http://localhost:11434/v1".into()),
            llm_api_key: optional("AXIOM_LLM_API_KEY"),
            llm_model: optional("AXIOM_LLM_MODEL"),
            telegram,
            db_path: optional("AXIOM_DB").unwrap_or_else(|| "axiom.db".into()),
            reports_dir: optional("AXIOM_REPORTS_DIR").unwrap_or_else(|| "reports".into()),
        })
    }
}

/// Reads tickers from a file: one per line, `#` comments, blank lines ignored.
pub fn load_watchlist(path: &str) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading watchlist {path}"))?;
    let mut out: Vec<String> = text
        .lines()
        .filter_map(|l| l.split('#').next().unwrap_or("").split_whitespace().next().map(str::to_uppercase))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Optional second column of a watchlist line: the date the stock joined the index (`AAPL 1982-11-30`).
/// Backtests treat a stock as eligible only from that date, so stocks that were added to the index
/// *because* they had already risen are not traded with hindsight before they joined.
pub fn load_added_dates(path: &str) -> Result<std::collections::HashMap<String, chrono::NaiveDate>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading watchlist {path}"))?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut it = l.split('#').next()?.split_whitespace();
            let sym = it.next()?.to_uppercase();
            let date = chrono::NaiveDate::parse_from_str(it.next()?, "%Y-%m-%d").ok()?;
            Some((sym, date))
        })
        .collect())
}
