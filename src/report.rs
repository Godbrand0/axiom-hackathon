//! Markdown research dossier. Everything above "Analyst note" is deterministic.
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;

use crate::models::{Signal, Stance};

pub fn render(signal: &Signal, setup_rules: &str, history: &str, note: Option<&str>, data_feed: &str) -> String {
    let mut s = String::new();
    let risk = signal.entry - signal.stop;
    let _ = writeln!(s, "# {} | {} | {}\n", signal.symbol, signal.setup, signal.as_of);
    let _ = writeln!(
        s,
        "> Research hypothesis, not financial advice. Levels are computed by code from daily bars ({data_feed} feed). \
         Paper fills assume the next session's open.\n"
    );

    let _ = writeln!(s, "## Trade plan\n");
    let _ = writeln!(s, "| | |\n|---|---|");
    let _ = writeln!(s, "| Direction | Long |");
    let _ = writeln!(s, "| Entry (ref: signal close) | {:.2} |", signal.entry);
    let _ = writeln!(s, "| Stop | {:.2} ({:.2}% risk) |", signal.stop, risk / signal.entry * 100.0);
    let _ = writeln!(
        s,
        "| Target | {:.2} (+{:.2}%) |",
        signal.target,
        (signal.target / signal.entry - 1.0) * 100.0
    );
    let _ = writeln!(s, "| Reward:risk | {:.2} |", signal.risk_reward());
    let _ = writeln!(s, "| Max hold | {} sessions |\n", signal.max_hold_days);
    let _ = writeln!(s, "**Setup rules:** {setup_rules}\n");

    let _ = writeln!(s, "## Historical record (backtest, net of costs)\n\n{history}\n");

    let _ = writeln!(s, "## Evidence\n");
    let _ = writeln!(s, "| ID | Kind | Stance | Finding | Source |\n|---|---|---|---|---|");
    for (i, e) in signal.evidence.iter().enumerate() {
        let stance = match e.stance {
            Stance::Supports => "supports",
            Stance::Opposes => "**opposes**",
            Stance::Neutral => "context",
        };
        let src = match (&e.url, &e.timestamp) {
            (Some(u), Some(t)) => format!("[{}]({}) {}", e.source, u, t),
            (Some(u), None) => format!("[{}]({})", e.source, u),
            (None, Some(t)) => format!("{} {}", e.source, t),
            (None, None) => e.source.clone(),
        };
        let kind = serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let _ = writeln!(s, "| E{} | {} | {} | {} | {} |", i + 1, kind, stance, e.text.replace('|', "/"), src);
    }

    let _ = writeln!(s, "\n## Analyst note\n");
    match note {
        Some(n) => {
            let _ = writeln!(s, "_Written by an LLM from the evidence above; check claims against cited IDs._\n\n{n}");
        }
        None => {
            let _ = writeln!(s, "_No LLM note (AXIOM_LLM_MODEL not set, --no-llm, or the call failed)._");
        }
    }
    s
}

pub fn write(dir: &str, signal: &Signal, content: &str) -> Result<PathBuf> {
    let folder = PathBuf::from(dir).join(signal.as_of.to_string());
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{}-{}.md", signal.symbol, signal.setup));
    std::fs::write(&path, content)?;
    Ok(path)
}
