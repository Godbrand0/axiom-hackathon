mod backtest;
mod cache;
mod config;
mod data;
mod factors;
mod fundamentals;
mod indicators;
mod models;
mod momentum;
mod pead;
mod report;
mod research;
mod setups;
mod store;
mod telegram;
mod universe;

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, TimeDelta, Utc};
use clap::{Parser, Subcommand};

use crate::backtest::{
    benchmark_forward_return, bootstrap_ci, buy_and_hold, mean_excluding_best, median, run_symbol, simulate_exit,
    stats, universe_forward_return, ExitReason, Trade,
};
use crate::config::{load_added_dates, load_watchlist, Config};
use crate::data::alpaca::Alpaca;
use crate::data::sec::{self, Sec};
use crate::indicators::Indicators;
use crate::models::{Bar, Evidence, EvidenceKind, Signal, Stance};
use crate::setups::Setup;
use crate::store::Store;

const BENCHMARK: &str = "SPY";

#[derive(Parser)]
#[command(name = "axiom", about = "Personal US-equity research agent (research only, no execution)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan the watchlist on the latest daily close, research hits, write reports, log paper trades.
    Scan {
        #[arg(long, default_value = "config/watchlist.txt")]
        watchlist: String,
        /// Override the watchlist, e.g. --symbols AAPL,MSFT
        #[arg(long, value_delimiter = ',')]
        symbols: Vec<String>,
        /// Years of history used for each signal's track record.
        #[arg(long, default_value_t = 8)]
        years: i64,
        #[arg(long)]
        no_llm: bool,
        #[arg(long)]
        no_telegram: bool,
        /// Round-trip cost assumption per side, in basis points.
        #[arg(long, default_value_t = 5.0)]
        cost_bps: f64,
    },
    /// Backtest every setup across the watchlist, with in-sample / out-of-sample split and SPY benchmark.
    Backtest {
        #[arg(long, default_value = "config/watchlist.txt")]
        watchlist: String,
        #[arg(long, value_delimiter = ',')]
        symbols: Vec<String>,
        #[arg(long, default_value_t = 8)]
        years: i64,
        #[arg(long, default_value_t = 5.0)]
        cost_bps: f64,
        /// Only run this setup.
        #[arg(long)]
        setup: Option<String>,
        /// Also write every simulated trade to this CSV file.
        #[arg(long)]
        csv: Option<String>,
    },
    /// Update paper trades for every logged signal and print the scorecard.
    Track {
        #[arg(long, default_value_t = 5.0)]
        cost_bps: f64,
    },
    /// Today's ranking for a monthly portfolio: momentum, quality, value or quality-value.
    #[command(alias = "momentum")]
    Rank {
        #[arg(long, default_value = "momentum")]
        by: String,
        #[arg(long, default_value = "config/sp500.txt")]
        watchlist: String,
        /// Fraction of the ranked universe to hold.
        #[arg(long, default_value_t = 0.1)]
        top: f64,
    },
    /// Recent earnings surprises (SUE) from 10-Q/10-K filings in the last N days.
    Earnings {
        #[arg(long, default_value = "config/sp500.txt")]
        watchlist: String,
        #[arg(long, default_value_t = 14)]
        days: i64,
    },
    /// Download an index constituent list to a watchlist file (currently: sp500).
    Universe {
        #[arg(default_value = "sp500")]
        index: String,
        #[arg(long, default_value = "config/sp500.txt")]
        out: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Universe { index, out } = &cli.cmd {
        return universe::download(index, out).await; // needs no API keys
    }
    let cfg = Config::from_env()?;
    match cli.cmd {
        Cmd::Universe { .. } => unreachable!(),
        Cmd::Scan { watchlist, symbols, years, no_llm, no_telegram, cost_bps } => {
            let (symbols, added) = resolve_symbols(&watchlist, symbols)?;
            scan(&cfg, &symbols, &added, years, !no_llm, !no_telegram, cost_bps).await
        }
        Cmd::Backtest { watchlist, symbols, years, cost_bps, setup, csv } => {
            let (symbols, added) = resolve_symbols(&watchlist, symbols)?;
            run_backtest(&cfg, &symbols, &added, years, cost_bps, setup.as_deref(), csv.as_deref()).await
        }
        Cmd::Track { cost_bps } => track(&cfg, cost_bps).await,
        Cmd::Rank { by, watchlist, top } => {
            rank_now(&cfg, &by, &load_watchlist(&watchlist)?, &load_added_dates(&watchlist)?, top).await
        }
        Cmd::Earnings { watchlist, days } => earnings_now(&cfg, &load_watchlist(&watchlist)?, days).await,
    }
}

/// Index-add date per symbol; empty when the watchlist has no dates or symbols were given explicitly.
type Added = HashMap<String, NaiveDate>;

fn resolve_symbols(watchlist: &str, symbols: Vec<String>) -> Result<(Vec<String>, Added)> {
    if symbols.is_empty() {
        Ok((load_watchlist(watchlist)?, load_added_dates(watchlist)?))
    } else {
        Ok((symbols.into_iter().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect(), Added::new()))
    }
}

/// Fetches adjusted daily bars for the symbols plus the benchmark.
/// End is 20 minutes ago because the free plan only serves SIP data older than 15 minutes.
/// Served from the local cache; only missing history / new days are downloaded.
async fn fetch_bars(
    cfg: &Config,
    alpaca: &Alpaca,
    symbols: &[String],
    start: chrono::DateTime<Utc>,
) -> Result<HashMap<String, Vec<Bar>>> {
    let mut all = symbols.to_vec();
    if !all.iter().any(|s| s == BENCHMARK) {
        all.push(BENCHMARK.into());
    }
    let end = Utc::now() - TimeDelta::minutes(20);
    let mut cache = cache::BarCache::open(&cfg.db_path)?;
    let bars = cache.get(alpaca, &all, start, end).await?;
    let missing: Vec<&String> = all.iter().filter(|s| !bars.contains_key(*s)).collect();
    if !missing.is_empty() {
        eprintln!(
            "No data for {} symbol(s): {}",
            missing.len(),
            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    Ok(bars)
}

/// Backtests one setup across all symbols, keeping only signals from while the stock was an index member.
fn setup_record(
    setup: &dyn Setup,
    bars: &HashMap<String, Vec<Bar>>,
    symbols: &[String],
    added: &Added,
    cost_bps: f64,
) -> Vec<Trade> {
    symbols
        .iter()
        .filter_map(|s| bars.get(s).map(|b| run_symbol(s, b, setup, cost_bps)))
        .flatten()
        .filter(|t| added.get(&t.symbol).is_none_or(|&a| t.signal_date >= a))
        .collect()
}

/// Stats plus the robustness checks that decide whether an edge is real:
/// SPY baseline, same-stocks random-entry baseline, outlier dependence, and a confidence interval.
fn edge_line(trades: &[&Trade], spy: &[Bar], bars: &HashMap<String, Vec<Bar>>, symbols: &[String], added: &Added) -> String {
    let st = stats(trades.iter().copied());
    if st.n == 0 {
        return "no completed trades".into();
    }
    let pct = |x: f64| format!("{:+.2}%", x * 100.0);
    let rets: Vec<f64> = trades.iter().map(|t| t.ret).collect();
    let from = trades.iter().map(|t| t.signal_date).min().unwrap();
    let to = trades.iter().map(|t| t.signal_date).max().unwrap();
    let hold = st.avg_hold.round() as usize;

    let mut out = st.line();
    out.push_str(&format!(
        "\n  median {} | mean without best 5 trades {}",
        median(&rets).map(pct).unwrap_or("-".into()),
        mean_excluding_best(&rets, 5).map(pct).unwrap_or("-".into()),
    ));
    if let Some((lo, hi)) = bootstrap_ci(&rets, 2000) {
        out.push_str(&format!(" | 90% CI of mean [{}, {}]", pct(lo), pct(hi)));
    }
    if let Some(b) = benchmark_forward_return(spy, hold, from, to) {
        out.push_str(&format!("\n  vs SPY held {hold} sessions: {}/hold -> edge {}", pct(b), pct(st.expectancy - b)));
    }
    if let Some(u) = universe_forward_return(bars, symbols, hold, from, to, added) {
        out.push_str(&format!(
            "\n  vs same stocks, random day, held {hold} sessions: {}/hold -> edge {} => {}",
            pct(u),
            pct(st.expectancy - u),
            verdict(&rets, u)
        ));
        // Diagnostic: same entries, but ignore stop/target/rule and just hold `hold` sessions.
        // Separates "the entries are bad" from "the exits are bad".
        let fwd: Vec<f64> = trades
            .iter()
            .filter_map(|t| {
                let b = bars.get(&t.symbol)?;
                let ei = b.binary_search_by_key(&t.entry_date, |x| x.date).ok()?;
                Some(b.get(ei + hold.max(1) - 1)?.close / t.entry - 1.0)
            })
            .collect();
        if !fwd.is_empty() {
            let m = fwd.iter().sum::<f64>() / fwd.len() as f64;
            out.push_str(&format!(
                "\n  entries only (no exits, held {hold}, before costs): {}/hold -> edge {} => {}",
                pct(m),
                pct(m - u),
                verdict(&fwd, u)
            ));
        }
    }
    out
}

fn verdict(values: &[f64], baseline: f64) -> &'static str {
    match bootstrap_ci(values, 2000) {
        Some((lo, _)) if lo > baseline => "EDGE (90% CI clears baseline)",
        Some((_, hi)) if hi < baseline => "WORSE than random entry",
        _ => "not distinguishable from random entry",
    }
}

async fn scan(cfg: &Config, symbols: &[String], added: &Added, years: i64, use_llm: bool, use_telegram: bool, cost_bps: f64) -> Result<()> {
    let alpaca = Alpaca::new(cfg);
    let store = Store::open(&cfg.db_path)?;
    let start = Utc::now() - TimeDelta::days(365 * years + 30);
    let bars = fetch_bars(cfg, &alpaca, symbols, start).await?;
    let spy = bars.get(BENCHMARK).context("no benchmark (SPY) bars returned")?;
    let latest = spy.last().context("empty SPY series")?.date;
    eprintln!("Latest session: {latest}");

    // Market regime evidence, shared by every signal today.
    let spy_ind = Indicators::compute(spy);
    let last = spy.len() - 1;
    let regime = match spy_ind.sma200[last] {
        Some(sma) => {
            let above = spy[last].close > sma;
            Evidence {
                kind: EvidenceKind::Market,
                stance: if above { Stance::Supports } else { Stance::Opposes },
                text: format!(
                    "Market regime: SPY {:.2} is {} its 200-day SMA {:.2}",
                    spy[last].close,
                    if above { "above" } else { "below" },
                    sma
                ),
                source: "Axiom indicator engine (SPY daily bars)".into(),
                url: None,
                timestamp: Some(latest.to_string()),
            }
        }
        None => Evidence::technical(Stance::Neutral, "Market regime unknown (insufficient SPY history)"),
    };

    // 1) Deterministic scan on the latest bar.
    let all_setups = setups::all();
    let mut hits: Vec<(Signal, &dyn Setup)> = Vec::new();
    for sym in symbols {
        let Some(b) = bars.get(sym) else {
            eprintln!("  {sym}: no data, skipped");
            continue;
        };
        if b.last().map(|x| x.date) != Some(latest) {
            eprintln!("  {sym}: stale data (last bar {:?}), skipped", b.last().map(|x| x.date));
            continue;
        }
        let ind = Indicators::compute(b);
        for setup in &all_setups {
            if let Some(sig) = setup.evaluate(sym, b, &ind, b.len() - 1) {
                hits.push((sig, setup.as_ref()));
            }
        }
    }
    eprintln!("{} setup hit(s) on {latest} across {} symbols.", hits.len(), symbols.len());
    if hits.is_empty() {
        return Ok(());
    }

    // 2) Track record per setup, computed once.
    let mut records: HashMap<&str, Vec<Trade>> = HashMap::new();
    for (_, setup) in &hits {
        records.entry(setup.name()).or_insert_with(|| setup_record(*setup, &bars, symbols, added, cost_bps));
    }

    // 3) Research each hit.
    let sec_client = match Sec::new(&cfg.sec_user_agent).await {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("  warning: SEC unavailable ({e:#}); reports will lack filings");
            None
        }
    };
    let researcher = match (&cfg.llm_model, use_llm) {
        (Some(model), true) => {
            Some(research::Researcher::new(&cfg.llm_base_url, cfg.llm_api_key.clone(), model.clone()))
        }
        _ => None,
    };

    for (mut sig, setup) in hits {
        if store.exists(&sig.symbol, &sig.setup, sig.as_of)? {
            eprintln!("  {} {}: already reported for {}, skipped", sig.symbol, sig.setup, sig.as_of);
            continue;
        }
        eprintln!("  Researching {} ({})...", sig.symbol, sig.setup);
        sig.evidence.push(regime.clone());

        if let Some(sec) = &sec_client {
            match sec.recent_filings(&sig.symbol, latest - TimeDelta::days(30)).await {
                Ok(filings) => add_filing_evidence(&mut sig, &filings),
                Err(e) => eprintln!("    SEC filings failed: {e:#}"),
            }
        }
        match alpaca.news(&sig.symbol, Utc::now() - TimeDelta::days(7), 10).await {
            Ok(news) => {
                for n in news {
                    let summary = if n.summary.is_empty() { String::new() } else { format!(" -- {}", n.summary) };
                    sig.evidence.push(Evidence {
                        kind: EvidenceKind::News,
                        stance: Stance::Neutral,
                        text: format!("Headline: {}{}", n.headline, summary).chars().take(600).collect(),
                        source: if n.source.is_empty() { "Alpaca News".into() } else { n.source },
                        url: (!n.url.is_empty()).then_some(n.url),
                        timestamp: Some(n.published_at.to_rfc3339()),
                    });
                }
            }
            Err(e) => eprintln!("    news failed: {e:#}"),
        }

        let all_trades = &records[setup.name()];
        let own: Vec<&Trade> = all_trades.iter().filter(|t| t.symbol == sig.symbol).collect();
        let every: Vec<&Trade> = all_trades.iter().collect();
        let history = format!(
            "- On {}: {}\n- Across the {}-symbol watchlist: {}\n- Caveats: survivorship bias (today's watchlist only), {cost_bps} bps/side costs, \
             daily bars only (intraday path unknown; stop assumed hit before target on the same day). Past results do not guarantee future results.",
            sig.symbol,
            stats(own.iter().copied()).line(),
            symbols.len(),
            edge_line(&every, spy, &bars, symbols, added).replace('\n', "\n  "),
        );

        let note = match &researcher {
            Some(r) => match r.write_note(&sig, setup.description(), &history).await {
                Ok(n) => Some(n),
                Err(e) => {
                    eprintln!("    LLM note failed: {e:#}");
                    None
                }
            },
            None => None,
        };

        let md = report::render(&sig, setup.description(), &history, note.as_deref(), &cfg.alpaca_feed);
        let path = report::write(&cfg.reports_dir, &sig, &md)?;
        store.insert(&sig, &path.to_string_lossy())?;
        println!(
            "{} {} | entry ~{:.2} stop {:.2} target {:.2} (R:R {:.1}) -> {}",
            sig.symbol, sig.setup, sig.entry, sig.stop, sig.target, sig.risk_reward(), path.display()
        );

        if let (true, Some((token, chat))) = (use_telegram, &cfg.telegram) {
            let verdict = note
                .as_deref()
                .and_then(|n| n.split("## Verdict").nth(1))
                .map(|v| v.trim().lines().take(3).collect::<Vec<_>>().join(" "))
                .unwrap_or_else(|| "(no analyst note)".into());
            let opposing = sig.evidence.iter().filter(|e| e.stance == Stance::Opposes).count();
            let msg = format!(
                "Axiom | {} {} ({})\nEntry ~{:.2}  Stop {:.2}  Target {:.2}  R:R {:.1}\nOpposing evidence items: {}\nHistory on {}: {}\nVerdict: {}\nReport: {}\n\nResearch only, not advice.",
                sig.symbol,
                sig.setup,
                sig.as_of,
                sig.entry,
                sig.stop,
                sig.target,
                sig.risk_reward(),
                opposing,
                sig.symbol,
                stats(own.iter().copied()).line(),
                verdict,
                path.display()
            );
            if let Err(e) = telegram::send(token, chat, &msg).await {
                eprintln!("    Telegram failed: {e:#}");
            }
        }
    }
    Ok(())
}

fn add_filing_evidence(sig: &mut Signal, filings: &[models::Filing]) {
    let insider = filings.iter().filter(|f| f.form == "4").count();
    if insider > 0 {
        sig.evidence.push(Evidence {
            kind: EvidenceKind::Filing,
            stance: Stance::Neutral,
            text: format!("{insider} insider (Form 4) filing(s) in the last 30 days; buy/sell direction not parsed yet"),
            source: "SEC EDGAR".into(),
            url: None,
            timestamp: None,
        });
    }
    for f in filings.iter().filter(|f| f.form != "4") {
        let codes: Vec<&str> = f.items.split(',').map(str::trim).filter(|c| !c.is_empty()).collect();
        let red = codes.iter().any(|c| sec::is_red_flag(c));
        let desc = if codes.is_empty() {
            String::new()
        } else {
            format!(": {}", codes.iter().map(|c| format!("{c} {}", sec::item_description(c))).collect::<Vec<_>>().join("; "))
        };
        let dilution = matches!(f.form.as_str(), "S-3" | "424B5" | "S-1");
        sig.evidence.push(Evidence {
            kind: EvidenceKind::Filing,
            stance: if red || dilution { Stance::Opposes } else { Stance::Neutral },
            text: format!(
                "{} filed {}{}{}",
                f.form,
                f.filed,
                desc,
                if dilution { " (possible share issuance / dilution)" } else { "" }
            ),
            source: format!("SEC EDGAR {}", f.form),
            url: Some(f.url.clone()),
            timestamp: Some(f.filed.to_string()),
        });
    }
}

async fn run_backtest(
    cfg: &Config,
    symbols: &[String],
    added: &Added,
    years: i64,
    cost_bps: f64,
    only: Option<&str>,
    csv_path: Option<&str>,
) -> Result<()> {
    let mut csv = String::from("setup,symbol,sample,signal_date,entry_date,exit_date,entry,exit,reason,ret_pct,r_multiple,hold_days\n");
    let alpaca = Alpaca::new(cfg);
    let start = Utc::now() - TimeDelta::days(365 * years + 30);
    let bars = fetch_bars(cfg, &alpaca, symbols, start).await?;
    let spy = bars.get(BENCHMARK).context("no SPY bars")?;
    let (first, last) = (spy.first().context("empty SPY")?.date, spy.last().unwrap().date);
    // Indicators need ~252 sessions to warm up; split the remaining span 70/30.
    let warm = spy.get(252).map(|b| b.date).unwrap_or(first);
    let cutoff = warm + TimeDelta::days(((last - warm).num_days() as f64 * 0.7) as i64);

    println!("Universe: {} symbols | data {first} -> {last} | costs {cost_bps} bps/side", symbols.len());
    if added.is_empty() {
        println!("Index membership: unknown (no add dates in the watchlist) -> every stock counts from day one (hindsight bias)");
    } else {
        let late = added.values().filter(|&&d| d > first).count();
        println!(
            "Index membership: point-in-time add dates for {} symbols; {late} joined after {first} and only count from their add date",
            added.len()
        );
    }
    println!("In-sample: {warm} -> {cutoff} | Out-of-sample: {cutoff} -> {last}");
    println!(
        "SPY buy & hold: IS {:+.1}% | OOS {:+.1}%",
        buy_and_hold(spy, warm, cutoff).unwrap_or(0.0) * 100.0,
        buy_and_hold(spy, cutoff, last).unwrap_or(0.0) * 100.0
    );
    println!("Tune rules looking at in-sample only; out-of-sample is your honest estimate.\n");

    for setup in setups::all() {
        if only.is_some_and(|n| n != setup.name()) {
            continue;
        }
        let trades = setup_record(setup.as_ref(), &bars, symbols, added, cost_bps);
        for t in &trades {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{:.4},{:.4},{},{:.4},{:.3},{}\n",
                setup.name(),
                t.symbol,
                if t.signal_date < cutoff { "IS" } else { "OOS" },
                t.signal_date,
                t.entry_date,
                t.exit_date,
                t.entry,
                t.exit,
                t.reason.as_str(),
                t.ret * 100.0,
                t.r_multiple,
                t.hold_days
            ));
        }
        let is: Vec<&Trade> = trades.iter().filter(|t| t.signal_date < cutoff).collect();
        let oos: Vec<&Trade> = trades.iter().filter(|t| t.signal_date >= cutoff).collect();
        let reasons = |v: &[&Trade], r: ExitReason| v.iter().filter(|t| t.reason == r).count();
        println!("== {} ==\n{}", setup.name(), setup.description());
        println!("  IS : {}", edge_line(&is, spy, &bars, symbols, added).replace('\n', "\n       "));
        println!("  OOS: {}", edge_line(&oos, spy, &bars, symbols, added).replace('\n', "\n       "));
        println!(
            "  OOS exits: {} target / {} stop / {} rule / {} time\n",
            reasons(&oos, ExitReason::Target),
            reasons(&oos, ExitReason::Stop),
            reasons(&oos, ExitReason::Rule),
            reasons(&oos, ExitReason::Time)
        );
    }
    let want = |n: &str| only.is_none_or(|o| o == n);
    let print_portfolio = |name: &str, desc: &str, periods: &[momentum::Period]| {
        println!("== {name} ==\n{desc}");
        let is: Vec<&momentum::Period> = periods.iter().filter(|p| p.date >= warm && p.date < cutoff).collect();
        let oos: Vec<&momentum::Period> = periods.iter().filter(|p| p.date >= cutoff).collect();
        println!("  IS : {}", portfolio_line(&is));
        println!("  OOS: {}\n", portfolio_line(&oos));
    };
    if want(momentum::NAME) {
        print_portfolio(momentum::NAME, momentum::DESCRIPTION, &momentum::run(&bars, symbols, spy, 0.1, cost_bps, added));
    }

    // Fundamentals-based strategies (SEC XBRL, point-in-time by filing date).
    let factors: Vec<factors::Factor> = factors::Factor::ALL.into_iter().filter(|f| want(f.name())).collect();
    if !factors.is_empty() || want(pead::NAME) {
        let facts = load_facts(cfg, symbols, 30).await?;
        let eligible = |s: &str, d: NaiveDate| added.get(s).is_none_or(|&a| a <= d);
        if !factors.is_empty() {
            let raw = cache::BarCache::open(&cfg.db_path)?
                .raw_monthly_closes(&alpaca, symbols, start, Utc::now() - TimeDelta::minutes(20))
                .await?;
            for f in factors {
                let rank = |d: NaiveDate| {
                    let key = cache::month_key(d);
                    factors::ranking(f, &facts, symbols, d, &|s| eligible(s, d), &|s| raw.get(s)?.get(&key).copied())
                };
                print_portfolio(f.name(), f.description(), &momentum::run_ranked(&bars, spy, 0.1, cost_bps, &rank));
            }
        }
        if want(pead::NAME) {
            let mut events: Vec<pead::Event> = symbols
                .iter()
                .filter_map(|s| Some(pead::events(s, facts.get(s)?, bars.get(s)?, cost_bps)))
                .flatten()
                .filter(|e| e.filed >= warm && eligible(&e.symbol, e.filed))
                .collect();
            events.sort_by_key(|e| e.filed);
            println!("== {} ==\n{}", pead::NAME, pead::DESCRIPTION);
            println!("  {} quarterly surprises from {} companies", events.len(), facts.len());
            for (label, oos) in [("IS ", false), ("OOS", true)] {
                let sample: Vec<&pead::Event> = events.iter().filter(|e| (e.filed >= cutoff) == oos).collect();
                println!("  {label}:");
                let groups: [(&str, &dyn Fn(&pead::Event) -> bool, fn(&pead::Event) -> Option<f64>); 3] = [
                    ("beats, bought after the filing (what Axiom can act on)", &|e| e.sue >= pead::BEAT, |e| e.ret_after_filing),
                    ("misses, bought after the filing (drift predicts WORSE)", &|e| e.sue <= pead::MISS, |e| e.ret_after_filing),
                    ("beats, bought after the price reaction (needs press-release data)", &|e| e.sue >= pead::BEAT, |e| e.ret_after_reaction),
                ];
                for (name, pick, ret) in groups {
                    let chosen: Vec<(&pead::Event, f64)> =
                        sample.iter().filter(|e| pick(e)).filter_map(|e| Some((*e, ret(e)?))).collect();
                    println!("    {name}\n      {}", event_line(&chosen, &bars, symbols, added).replace('\n', "\n      "));
                }
            }
            println!();
        }
    }
    println!(
        "Caveat: survivorship bias. Stocks removed from the index (often after falling) are missing, so results are still somewhat optimistic."
    );
    if let Some(p) = csv_path {
        std::fs::write(p, csv).with_context(|| format!("writing {p}"))?;
        println!("Trades written to {p}");
    }
    Ok(())
}

fn portfolio_line(periods: &[&momentum::Period]) -> String {
    if periods.is_empty() {
        return "no complete months".into();
    }
    let n = periods.len() as f64;
    let mean = |f: fn(&momentum::Period) -> f64| periods.iter().map(|p| f(p)).sum::<f64>() / n;
    let total = |f: fn(&momentum::Period) -> f64| periods.iter().fold(1.0, |acc, p| acc * (1.0 + f(p))) - 1.0;
    let excess: Vec<f64> = periods.iter().map(|p| p.top - p.universe).collect();
    let beat = excess.iter().filter(|&&x| x > 0.0).count() as f64 / n;
    let pct = |x: f64| format!("{:+.2}%", x * 100.0);
    let ci = bootstrap_ci(&excess, 2000);
    let verdict = match ci {
        Some((lo, _)) if lo > 0.0 => "EDGE (90% CI clears equal-weight universe)",
        Some((_, hi)) if hi < 0.0 => "WORSE than equal-weight universe",
        _ => "not distinguishable from equal-weight universe",
    };
    format!(
        "{} months | top 10% {}/mo (total {}) | equal-weight universe {}/mo (total {}) | SPY {}/mo (total {})\n         \
         excess vs universe {}/mo{} | beat universe in {:.0}% of months => {verdict}",
        periods.len(),
        pct(mean(|p| p.top)),
        pct(total(|p| p.top)),
        pct(mean(|p| p.universe)),
        pct(total(|p| p.universe)),
        pct(mean(|p| p.spy)),
        pct(total(|p| p.spy)),
        pct(excess.iter().sum::<f64>() / n),
        ci.map(|(lo, hi)| format!(", 90% CI [{}, {}]", pct(lo), pct(hi))).unwrap_or_default(),
        beat * 100.0,
    )
}

/// Per-event returns vs buying the same stocks on a random day for the same holding period.
fn event_line(chosen: &[(&pead::Event, f64)], bars: &HashMap<String, Vec<Bar>>, symbols: &[String], added: &Added) -> String {
    if chosen.len() < 2 {
        return format!("{} events", chosen.len());
    }
    let pct = |x: f64| format!("{:+.2}%", x * 100.0);
    let rets: Vec<f64> = chosen.iter().map(|(_, r)| *r).collect();
    let n = rets.len() as f64;
    let mean = rets.iter().sum::<f64>() / n;
    let win = rets.iter().filter(|&&r| r > 0.0).count() as f64 / n;
    let mut out = format!(
        "{} events | mean {}/trade | median {} | win {:.0}%",
        rets.len(),
        pct(mean),
        median(&rets).map(pct).unwrap_or_default(),
        win * 100.0
    );
    if let Some((lo, hi)) = bootstrap_ci(&rets, 2000) {
        out.push_str(&format!(" | 90% CI [{}, {}]", pct(lo), pct(hi)));
    }
    let from = chosen.iter().map(|(e, _)| e.filed).min().unwrap();
    let to = chosen.iter().map(|(e, _)| e.filed).max().unwrap();
    if let Some(u) = universe_forward_return(bars, symbols, pead::HOLD, from, to, added) {
        out.push_str(&format!(
            "\nvs same stocks, random day, held {}: {} -> edge {} => {}",
            pead::HOLD,
            pct(u),
            pct(mean - u),
            verdict(&rets, u)
        ));
    }
    out
}

async fn load_facts(cfg: &Config, symbols: &[String], max_age_days: i64) -> Result<HashMap<String, fundamentals::Facts>> {
    let sec = Sec::new(&cfg.sec_user_agent).await.context("connecting to SEC EDGAR")?;
    let mut store = fundamentals::FactStore::open(&cfg.db_path)?;
    store.refresh(&sec, symbols, max_age_days).await?;
    let facts = store.load_all(symbols)?;
    eprintln!("Fundamentals: SEC data for {} of {} symbols", facts.len(), symbols.len());
    Ok(facts)
}

/// Prints today's ranking: what the monthly portfolio would hold if it rebalanced on the latest close.
async fn rank_now(cfg: &Config, by: &str, symbols: &[String], added: &Added, top: f64) -> Result<()> {
    let factor = match by {
        momentum::NAME | "momentum" => None,
        other => Some(factors::Factor::parse(other).with_context(|| {
            format!("unknown ranking '{other}' (use momentum, quality, value or quality-value)")
        })?),
    };
    let alpaca = Alpaca::new(cfg);
    let bars = fetch_bars(cfg, &alpaca, symbols, Utc::now() - TimeDelta::days(400)).await?;
    let spy = bars.get(BENCHMARK).context("no SPY bars")?;
    let latest = spy.last().context("empty SPY series")?.date;
    let (name, ranked) = match factor {
        None => (momentum::NAME, momentum::ranking(&bars, symbols, latest, added)),
        Some(f) => {
            let facts = load_facts(cfg, symbols, 7).await?;
            // The latest bar is not yet adjusted for anything, so its close is the quoted price.
            let price = |s: &str| bars.get(s)?.last().filter(|b| b.date == latest).map(|b| b.close);
            (f.name(), factors::ranking(f, &facts, symbols, latest, &|_| true, &price))
        }
    };
    let n = momentum::top_count(ranked.len(), top);
    let unit = match factor {
        None => "12-1 return",
        Some(factors::Factor::Quality) => "TTM op. cash flow / assets",
        Some(factors::Factor::Value) => "earnings yield (TTM EPS / price)",
        Some(factors::Factor::QualityValue) => "combined percentile rank",
    };
    println!("{name} ranking on {latest} ({} eligible stocks); top {n} by {unit}:", ranked.len());
    for (i, (s, sc)) in ranked.iter().take(n).enumerate() {
        let shown = match factor {
            Some(factors::Factor::QualityValue) => format!("{:.2}", sc),
            Some(factors::Factor::Value) if *sc > 0.0 => format!("{:+.1}%  (P/E {:.1})", sc * 100.0, 1.0 / sc),
            _ => format!("{:+.1}%", sc * 100.0),
        };
        println!("  {:>3}. {:<6} {}", i + 1, s, shown);
    }
    println!(
        "\nThe backtested rule rebalances only at month-end (buy at the next open, hold a month).\n\
         Run `backtest --watchlist config/sp500.txt --setup {name}` to see whether it beat the equal-weight universe. \
         Research only, not advice."
    );
    Ok(())
}

/// Earnings surprises filed in the last `days` days, strongest first.
async fn earnings_now(cfg: &Config, symbols: &[String], days: i64) -> Result<()> {
    if days < 1 {
        bail!("--days must be at least 1");
    }
    let alpaca = Alpaca::new(cfg);
    let bars = fetch_bars(cfg, &alpaca, symbols, Utc::now() - TimeDelta::days(400)).await?;
    let facts = load_facts(cfg, symbols, 1).await?;
    let since = Utc::now().date_naive() - TimeDelta::days(days);
    let mut recent: Vec<pead::Event> = symbols
        .iter()
        .filter_map(|s| Some(pead::events(s, facts.get(s)?, bars.get(s)?, 0.0)))
        .flatten()
        .filter(|e| e.filed >= since)
        .collect();
    recent.sort_by(|a, b| b.sue.total_cmp(&a.sue));
    println!("Earnings surprises filed since {since} ({} filings):", recent.len());
    println!("{:<7} {:<11} {:<11} {:>9} {:>11} {:>7}  {:<11} {:>7}  SIGNAL", "SYMBOL", "QUARTER", "FILED", "EPS", "YEAR AGO", "SUE", "REACTION", "GAP");
    for e in &recent {
        let signal = if e.sue >= pead::BEAT {
            "strong beat"
        } else if e.sue <= pead::MISS {
            "strong miss"
        } else {
            ""
        };
        println!(
            "{:<7} {:<11} {:<11} {:>9.2} {:>11.2} {:>7.1}  {:<11} {:>7}  {signal}",
            e.symbol,
            e.period_end,
            e.filed,
            e.eps,
            e.eps_year_ago,
            e.sue,
            e.reaction.map(|d| d.to_string()).unwrap_or_else(|| "-".into()),
            e.reaction_gap.map(|g| format!("{:+.1}%", g * 100.0)).unwrap_or_else(|| "-".into()),
        );
    }
    println!(
        "\nSUE = (EPS - same quarter last year - its usual change) / its usual variability. The backtest holds strong beats 60 sessions.\n\
         Check `backtest --watchlist config/sp500.txt --setup {}` before acting on these. Research only, not advice.",
        pead::NAME
    );
    Ok(())
}

async fn track(cfg: &Config, cost_bps: f64) -> Result<()> {
    let store = Store::open(&cfg.db_path)?;
    let mut signals = store.all()?;
    let pending: Vec<&mut store::StoredSignal> =
        signals.iter_mut().filter(|s| s.status == "open" || s.status == "filled").collect();
    if pending.is_empty() && store.all()?.is_empty() {
        println!("No signals logged yet. Run `axiom scan` first.");
        return Ok(());
    }

    let alpaca = Alpaca::new(cfg);
    let mut syms: Vec<String> = pending.iter().map(|s| s.symbol.clone()).collect();
    syms.sort();
    syms.dedup();
    let earliest = pending.iter().map(|s| s.as_of).min().unwrap_or_else(|| Utc::now().date_naive());
    let start = earliest.and_hms_opt(0, 0, 0).unwrap().and_utc() - TimeDelta::days(5);
    let bars = fetch_bars(cfg, &alpaca, &syms, start).await?;
    let cost = 2.0 * cost_bps / 10_000.0;

    for s in pending {
        let Some(b) = bars.get(&s.symbol) else { continue };
        let Some(ei) = b.iter().position(|x| x.date > s.as_of) else { continue }; // not filled yet
        let fill = b[ei].open;
        if s.status == "open" && (fill <= s.stop || fill >= s.target) {
            s.status = "skipped_gap".into();
            store.update(s)?;
            continue;
        }
        s.fill_date = Some(b[ei].date);
        s.fill_price = Some(fill);
        let setup = setups::by_name(&s.setup);
        let ind = Indicators::compute(b);
        let exit = simulate_exit(b, ei, s.stop, s.target, s.max_hold, &|j| {
            setup.as_ref().is_some_and(|st| st.exit_on_close(b, &ind, j))
        });
        if exit.reason == ExitReason::Open {
            s.status = "filled".into();
            s.exit_price = Some(exit.price);
            s.ret = Some(exit.price / fill - 1.0);
        } else {
            s.status = exit.reason.as_str().into();
            s.exit_date = Some(b[exit.idx].date);
            s.exit_price = Some(exit.price);
            s.ret = Some(exit.price / fill - 1.0 - cost);
        }
        store.update(s)?;
    }

    // Scorecard
    let spy = bars.get(BENCHMARK);
    let all = store.all()?;
    println!("{:<7} {:<15} {:<10} {:<11} {:>8} {:>8}  {}", "SYMBOL", "SETUP", "SIGNAL", "STATUS", "RET", "vs SPY", "EXIT");
    let mut closed = Vec::new();
    for s in &all {
        let spy_ret = match (spy, s.fill_date) {
            (Some(spy), Some(fd)) => window_return(spy, fd, s.exit_date.unwrap_or_else(|| Utc::now().date_naive())),
            _ => None,
        };
        let ret = s.ret.map(|r| format!("{:+.2}%", r * 100.0)).unwrap_or_else(|| "-".into());
        let rel = match (s.ret, spy_ret) {
            (Some(r), Some(b)) => format!("{:+.2}%", (r - b) * 100.0),
            _ => "-".into(),
        };
        let status = if s.status == "filled" { "open*".to_string() } else { s.status.clone() };
        println!(
            "{:<7} {:<15} {:<10} {:<11} {:>8} {:>8}  {}",
            s.symbol,
            s.setup,
            s.as_of,
            status,
            ret,
            rel,
            s.exit_date.map(|d| d.to_string()).unwrap_or_default()
        );
        if matches!(s.status.as_str(), "stop" | "target" | "time" | "rule") {
            if let Some(r) = s.ret {
                closed.push((r, spy_ret));
            }
        }
    }
    if !closed.is_empty() {
        let n = closed.len() as f64;
        let wins = closed.iter().filter(|(r, _)| *r > 0.0).count() as f64;
        let avg = closed.iter().map(|(r, _)| r).sum::<f64>() / n;
        let edges: Vec<f64> = closed.iter().filter_map(|(r, b)| b.map(|b| r - b)).collect();
        println!(
            "\nClosed paper trades: {} | win {:.0}% | avg {:+.2}%/trade{}",
            closed.len(),
            wins / n * 100.0,
            avg * 100.0,
            if edges.is_empty() {
                String::new()
            } else {
                format!(" | avg edge vs SPY {:+.2}%", edges.iter().sum::<f64>() / edges.len() as f64 * 100.0)
            }
        );
    }
    println!("(open* = filled, unrealized; returns net of {cost_bps} bps/side on closed trades)");
    Ok(())
}

/// SPY return from the open of `from` to the close of `to`.
fn window_return(spy: &[Bar], from: NaiveDate, to: NaiveDate) -> Option<f64> {
    let a = spy.iter().find(|b| b.date >= from)?;
    let z = spy.iter().rev().find(|b| b.date <= to)?;
    Some(z.close / a.open - 1.0)
}
