use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// One daily OHLCV bar (split/dividend adjusted).
#[derive(Debug, Clone, Serialize)]
pub struct Bar {
    pub date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewsItem {
    pub headline: String,
    pub summary: String,
    pub published_at: DateTime<Utc>,
    pub url: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filing {
    pub form: String,
    pub filed: NaiveDate,
    /// 8-K item codes, e.g. "2.02,9.01". Empty for other forms.
    pub items: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stance {
    Supports,
    Opposes,
    /// Context the code cannot score on its own (e.g. a headline). The LLM may interpret it, labelled as interpretation.
    Neutral,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceKind {
    Technical,
    Market,
    Filing,
    News,
}

/// A single piece of evidence. Numbers in `text` are computed by code, never by the LLM.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub stance: Stance,
    pub text: String,
    pub source: String,
    pub url: Option<String>,
    pub timestamp: Option<String>,
}

impl Evidence {
    pub fn technical(stance: Stance, text: impl Into<String>) -> Self {
        Self {
            kind: EvidenceKind::Technical,
            stance,
            text: text.into(),
            source: "Axiom indicator engine (daily bars)".into(),
            url: None,
            timestamp: None,
        }
    }
}

/// A trade idea produced deterministically by a setup. Entry/stop/target are code-computed.
#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub symbol: String,
    pub setup: String,
    pub as_of: NaiveDate,
    /// Planned entry reference (signal-day close). Paper fills use the next session's open.
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub max_hold_days: usize,
    pub evidence: Vec<Evidence>,
}

impl Signal {
    pub fn risk_reward(&self) -> f64 {
        (self.target - self.entry) / (self.entry - self.stop)
    }
}
