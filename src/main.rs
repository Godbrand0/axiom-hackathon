mod backtest;
mod cache;
mod config;
mod data;
mod factors;
mod fundamentals;
mod indicators;
mod meme;
mod models;
mod momentum;
mod near;
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
use crate::near::client::sha256_hex;
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
        /// Commit each new signal's hash to the Axiom Pay contract on NEAR, and buy SEC evidence from
        /// AXIOM_SEC_PROVIDER_URL (paid per call on-chain) when that is set.
        #[arg(long)]
        near: bool,
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
    /// Memecoin research on NEAR: rug-risk screens, momentum signals committed on-chain, verification.
    Meme {
        /// Sign as this account (for --near commitments and buying).
        #[arg(long = "as", global = true)]
        as_account: Option<String>,
        #[command(subcommand)]
        cmd: meme::cli::MemeCmd,
    },
    /// Axiom Pay on NEAR: agent budget and policy, paid evidence providers, on-chain signal commitments.
    Near {
        /// Sign as this account (key from ~/.near-credentials/<network>/<account>.json). Default: NEAR_ACCOUNT_ID.
        #[arg(long = "as", global = true)]
        as_account: Option<String>,
        #[command(subcommand)]
        cmd: NearCmd,
    },
}

#[derive(Subcommand)]
enum NearCmd {
    /// Create a funded testnet account through the NEAR faucet and save its key.
    CreateAccount { account: String },
    /// Deploy the Axiom Pay marketplace contract to the --as account (and initialise it).
    Deploy {
        #[arg(long, default_value = "contract/target/near/axiom_pay.wasm")]
        wasm: String,
        /// Upgrade the code only, keeping the existing state.
        #[arg(long)]
        no_init: bool,
        /// Upgrade the code and migrate v2 state to v3 in the same transaction.
        #[arg(long, conflicts_with = "no_init")]
        migrate: bool,
    },
    /// Marketplace overview: services, prices, sales; plus the --as account's wallet if it has one.
    Status,
    /// Owner: open a wallet for an agent, funded with --budget NEAR, with spending caps.
    OpenWallet {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        budget: String,
        #[arg(long, default_value = "0.05")]
        max_per_call: String,
        #[arg(long, default_value = "0.5")]
        daily_cap: String,
    },
    /// Owner: add NEAR to your wallet's budget.
    Fund { amount: String },
    /// Owner: change your wallet's per-call and daily caps (in NEAR).
    Policy {
        #[arg(long)]
        max_per_call: String,
        #[arg(long)]
        daily_cap: String,
    },
    /// Seller: register or reprice a service (pays its own storage, excess refunded).
    Register {
        #[arg(long)]
        service: String,
        #[arg(long)]
        price: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Owner: let your agent pay a service at its current on-chain price.
    Approve { service: String },
    /// Owner: stop your agent paying a service.
    Revoke { service: String },
    /// Recent payment receipts across the marketplace.
    Receipts {
        #[arg(long, default_value_t = 20)]
        last: u64,
    },
    /// Seller: serve every service the --as account registered (Axiom research and/or SEC evidence),
    /// and optionally bid on jobs, e.g. `--bid sec-red-flags=0.015`.
    Serve {
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// Bid on jobs of this kind at this price (repeatable): sec-red-flags, axiom-signals, axiom-report.
        #[arg(long = "bid", value_name = "KIND=NEAR")]
        bids: Vec<String>,
        /// Base URL buyers fetch job deliveries from (default http://127.0.0.1:<port>).
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Owner: create an agent account `<name>.<you>` with its own key, restricted on-chain to the marketplace.
    ///
    /// The agent's key is a function-call key: it can only call the marketplace methods of its role, cannot
    /// attach NEAR, and has a gas allowance. Your own key is added as the account's recovery key. Buyers are
    /// added to your wallet (opened with --budget if you have none); storage credit is prepaid for both roles.
    AgentCreate {
        name: String,
        #[arg(long, value_parser = ["buyer", "seller", "both"])]
        role: String,
        /// Buyer: open your wallet with this budget if you have none, or add it to the existing budget.
        #[arg(long)]
        budget: Option<String>,
        #[arg(long, default_value = "0.05")]
        max_per_call: String,
        #[arg(long, default_value = "0.5")]
        daily_cap: String,
        /// NEAR sent to the agent account for gas (and as its key's gas allowance).
        #[arg(long, default_value = "0.2")]
        gas: String,
        /// Storage credit prepaid on the marketplace for the agent.
        #[arg(long, default_value = "0.05")]
        storage: String,
    },
    /// Prepay marketplace storage for an account (e.g. an agent whose key cannot attach NEAR).
    FundStorage {
        account: String,
        #[arg(long, default_value = "0.05")]
        amount: String,
    },
    /// Owner: your wallet's agents, with their balances and storage credit.
    Agents,
    /// Owner: move an agent account's NEAR (e.g. a seller's earnings) to you, signing with your recovery key.
    AgentSweep {
        agent: String,
        /// Leave this much on the agent for gas.
        #[arg(long, default_value = "0.1")]
        keep: String,
    },
    /// Buyer agent: post a job, take the best bid from a seller with a clean record, pay, collect, confirm.
    Hire {
        /// e.g. sec-red-flags|JPM|2026-10-10, axiom-signals|2026-11-09, axiom-report|JPM|2026-11-09
        topic: String,
        #[arg(long, default_value = "0.03")]
        max_price: String,
        /// Seconds to wait for bids.
        #[arg(long, default_value_t = 25)]
        wait: u64,
    },
    /// Recent jobs on the board with their bids and outcomes.
    Jobs {
        #[arg(long, default_value_t = 10)]
        last: u64,
    },
    /// Recent collective-purchase pools.
    Pools {
        #[arg(long, default_value_t = 10)]
        last: u64,
    },
    /// Demo economy: seller agents bid for buyer agents' jobs, then the buyers pool one purchase.
    Swarm {
        /// Seller agent and its price for sec-red-flags jobs (repeatable), e.g. --seller fast.alice.testnet=0.012
        #[arg(long = "seller", value_name = "ACCOUNT=NEAR", required = true)]
        sellers: Vec<String>,
        /// Buyer agent (repeatable).
        #[arg(long = "buyer", required = true)]
        buyers: Vec<String>,
        /// Symbols to hire SEC red-flag research for, one job each.
        #[arg(long, value_delimiter = ',', default_value = "JPM,XOM,NVDA")]
        symbols: Vec<String>,
        /// Days of filings each job asks for.
        #[arg(long, default_value_t = 30)]
        days: i64,
        #[arg(long, default_value = "0.03")]
        max_price: String,
        /// Collective purchase: all buyers split this seller's axiom-signals for --pool-date.
        #[arg(long)]
        pool_seller: Option<String>,
        #[arg(long)]
        pool_date: Option<NaiveDate>,
        #[arg(long, default_value_t = 8801)]
        base_port: u16,
    },
    /// Agent: buy one symbol's recent SEC filings from an evidence seller.
    Buy {
        symbol: String,
        #[arg(long, default_value = "http://127.0.0.1:8788")]
        provider: String,
        #[arg(long, default_value_t = 30)]
        days: i64,
    },
    /// Buyer agent: verify a seller's on-chain track record, and buy its latest signals only if it passes.
    Shop {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        seller: String,
        /// Session to buy (default: the seller's latest).
        #[arg(long)]
        date: Option<NaiveDate>,
        /// Refuse to buy unless at least this many past signals were committed before their entry.
        #[arg(long, default_value_t = 1)]
        min_verified: usize,
        /// Refuse to buy if more than this share of published signals are late or uncommitted.
        #[arg(long, default_value_t = 0.0)]
        max_unverified_share: f64,
        /// Also buy the full report for each signal.
        #[arg(long)]
        reports: bool,
    },
    /// Check a seller's published track record against its on-chain commitments (free; no key needed).
    CheckSeller {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        seller: String,
    },
    /// Agent: commit every ledger signal that is not on-chain yet (late ones will verify as LATE).
    CommitSignals,
    /// Seller: claim the escrowed payment for a job you won, once the buyer has been silent for a day.
    Claim { job_id: u64 },
    /// Seller: write the free track record (signals whose entry is known) to a JSON file, e.g. for the website.
    ExportRecord {
        #[arg(long, default_value = "web/public/track-record.json")]
        out: String,
    },
    /// Check every local ledger signal against the --as account's on-chain commitments.
    Verify,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Universe { index, out } = &cli.cmd {
        return universe::download(index, out).await; // needs no API keys
    }
    if let Cmd::Meme { as_account, cmd } = cli.cmd {
        let _ = dotenvy::dotenv();
        return meme::cli::run(as_account.as_deref(), cmd).await; // public data only, no market-data keys
    }
    if let Cmd::Near { as_account, cmd } = cli.cmd {
        let _ = dotenvy::dotenv();
        return near_cmd(as_account.as_deref(), cmd).await; // needs no market-data keys
    }
    let cfg = Config::from_env()?;
    match cli.cmd {
        Cmd::Universe { .. } | Cmd::Near { .. } | Cmd::Meme { .. } => unreachable!(),
        Cmd::Scan { watchlist, symbols, years, no_llm, no_telegram, cost_bps, near } => {
            let (symbols, added) = resolve_symbols(&watchlist, symbols)?;
            scan(&cfg, &symbols, &added, years, !no_llm, !no_telegram, cost_bps, near).await
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

#[allow(clippy::too_many_arguments)]
async fn scan(
    cfg: &Config,
    symbols: &[String],
    added: &Added,
    years: i64,
    use_llm: bool,
    use_telegram: bool,
    cost_bps: f64,
    use_near: bool,
) -> Result<()> {
    // Fail before any data work if NEAR is requested but not configured.
    let near_ctx = if use_near { Some(near::Ctx::from_env(None).context("--near")?) } else { None };
    let sec_provider = near_ctx.as_ref().and(std::env::var("AXIOM_SEC_PROVIDER_URL").ok()).filter(|s| !s.trim().is_empty());
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

        let since = latest - TimeDelta::days(30);
        if let (Some(ctx), Some(url)) = (&near_ctx, &sec_provider) {
            // Bought from a provider agent, paid per call through Axiom Pay.
            match near::buy_filings(ctx, url, &sig.symbol, since).await {
                Ok(filings) => add_filing_evidence(&mut sig, &filings),
                Err(e) => eprintln!("    paid SEC evidence failed: {e:#}"),
            }
        } else if let Some(sec) = &sec_client {
            match sec.recent_filings(&sig.symbol, since).await {
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
        if let Some(ctx) = &near_ctx {
            let hash = sha256_hex(&near::signal_line(&sig.symbol, &sig.setup, sig.as_of, sig.entry, sig.stop, sig.target, sig.max_hold_days));
            // Labels are public immediately: name only the session, never the symbol or setup being sold.
            let label = format!("axiom-signal/v1 {}", sig.as_of);
            match near::commit_signal(ctx, &hash, &label).await {
                Ok(tx) => {
                    let link = ctx.near.explorer_tx(&tx);
                    eprintln!("    committed on NEAR: {link}");
                    let line = format!(
                        "\n\n---\nOn-chain commitment: `sha256({})` = `{hash}` on `{}` ({}). Verify with `axiom near verify`.\n",
                        near::signal_line(&sig.symbol, &sig.setup, sig.as_of, sig.entry, sig.stop, sig.target, sig.max_hold_days),
                        ctx.contract,
                        link
                    );
                    use std::io::Write as _;
                    std::fs::OpenOptions::new().append(true).open(&path)?.write_all(line.as_bytes())?;
                }
                Err(e) => eprintln!("    NEAR commitment failed: {e:#}"),
            }
        }
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

/// `["sec-red-flags=0.015"]` -> {"sec-red-flags": yocto}.
fn parse_prices(items: &[String]) -> Result<std::collections::HashMap<String, u128>> {
    items
        .iter()
        .map(|b| {
            let (k, v) = b.split_once('=').with_context(|| format!("--bid takes KIND=NEAR, got {b:?}"))?;
            Ok((k.trim().to_string(), near::client::parse_near(v)?))
        })
        .collect()
}

async fn near_cmd(as_account: Option<&str>, cmd: NearCmd) -> Result<()> {
    use near::client::{fmt_near, parse_near, u128_field, Action, Near, Signer, TGAS};
    use serde_json::json;

    let near = Near::from_env();
    let signer = || Signer::load(&near.network, as_account);
    let contract = near::contract_id;
    let done = |what: &str, tx: &str| println!("{what}: {}", near.explorer_tx(tx));
    let db_path = || std::env::var("AXIOM_DB").unwrap_or_else(|_| "axiom.db".into());
    let me = || as_account.map(str::to_string).or_else(|| std::env::var("NEAR_ACCOUNT_ID").ok()).context("pass --as <account>");

    match cmd {
        NearCmd::CreateAccount { account } => {
            if near.network != "testnet" {
                bail!("the faucet only exists on testnet");
            }
            if near.balance(&account).await?.is_some() {
                bail!("{account} already exists");
            }
            let s = Signer::generate(&account)?;
            let resp = reqwest::Client::new()
                .post("https://helper.nearprotocol.com/account")
                .json(&json!({"newAccountId": account, "newAccountPublicKey": s.public_key_str()}))
                .send()
                .await?;
            if !resp.status().is_success() {
                bail!("faucet refused ({}): {}", resp.status(), resp.text().await.unwrap_or_default());
            }
            let path = s.save(&near.network)?;
            for _ in 0..10 {
                if let Some(b) = near.balance(&account).await? {
                    println!("Created {account} with {} (key saved to {})", fmt_near(b), path.display());
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            bail!("faucet accepted the request but {account} does not exist yet; key saved to {}", path.display())
        }
        NearCmd::Deploy { wasm, no_init, migrate } => {
            let s = signer()?;
            let code = std::fs::read(&wasm).with_context(|| format!("reading {wasm} (build it with contract/build.sh)"))?;
            let mut actions = vec![Action::Deploy(code)];
            if migrate {
                actions.push(Action::Call { method: "migrate".into(), args: json!({}), gas: 100 * TGAS, deposit: 0 });
            } else if !no_init {
                actions.push(Action::Call { method: "new".into(), args: json!({}), gas: 100 * TGAS, deposit: 0 });
            }
            let r = near.send(&s, &s.account_id, actions).await?;
            done(&format!("Deployed Axiom Pay to {}", s.account_id), &r.hash);
            println!("Add to .env:  AXIOM_PAY_CONTRACT={}", s.account_id);
        }
        NearCmd::Status => {
            let c = contract()?;
            let receipts = near.view(&c, "receipt_count", json!({})).await?;
            let commitments = near.view(&c, "commitment_count", json!({})).await?;
            println!("Axiom Pay marketplace {c} on {}: {receipts} payment(s), {commitments} commitment(s)", near.network);
            println!("Services:");
            for pair in near.view(&c, "list_services", json!({})).await?.as_array().into_iter().flatten() {
                let (id, s) = (&pair[0], &pair[1]);
                println!(
                    "  {:<14} {:>10}/call  seller {:<28} sold {:>3}  earned {}",
                    id.as_str().unwrap_or("?"),
                    fmt_near(u128_field(s, "price")?),
                    s["provider"].as_str().unwrap_or("?"),
                    s["calls"],
                    fmt_near(u128_field(s, "earned")?)
                );
            }
            if let Ok(acct) = me() {
                let mut w = near.view(&c, "get_wallet", json!({"owner": acct})).await?;
                if w.is_null() {
                    w = near.view(&c, "get_wallet_of_agent", json!({"agent": acct})).await?;
                }
                if !w.is_null() {
                    println!(
                        "Wallet of {} (agent {}): budget {}, spent today {} of {}, max {}/call, {} payment(s)",
                        w["owner"].as_str().unwrap_or("?"),
                        w["agent"].as_str().unwrap_or("?"),
                        fmt_near(u128_field(&w, "budget")?),
                        fmt_near(u128_field(&w, "spent_today")?),
                        fmt_near(u128_field(&w, "daily_cap")?),
                        fmt_near(u128_field(&w, "max_per_call")?),
                        w["payments"]
                    );
                }
            }
        }
        NearCmd::OpenWallet { agent, budget, max_per_call, daily_cap } => {
            let args = json!({"agent": agent, "max_per_call": parse_near(&max_per_call)?.to_string(), "daily_cap": parse_near(&daily_cap)?.to_string()});
            let r = near.call(&signer()?, &contract()?, "open_wallet", args, parse_near(&budget)?).await?;
            let b = r.value.as_ref().map(|w| u128_field(w, "budget")).transpose()?.unwrap_or(0);
            done(&format!("Wallet open for agent {agent}, budget {} after storage", fmt_near(b)), &r.hash);
        }
        NearCmd::Fund { amount } => {
            let r = near.call(&signer()?, &contract()?, "deposit", json!({}), parse_near(&amount)?).await?;
            let b: u128 = r.value.as_ref().and_then(|v| v.as_str()).unwrap_or("0").parse()?;
            done(&format!("Budget is now {}", fmt_near(b)), &r.hash);
        }
        NearCmd::Policy { max_per_call, daily_cap } => {
            let args = json!({"max_per_call": parse_near(&max_per_call)?.to_string(), "daily_cap": parse_near(&daily_cap)?.to_string()});
            let r = near.call(&signer()?, &contract()?, "set_policy", args, 0).await?;
            done("Policy updated", &r.hash);
        }
        NearCmd::Register { service, price, description } => {
            let description = description.unwrap_or_else(|| {
                match service.as_str() {
                    near::SIGNALS_SERVICE => "Axiom: one session's fresh signals (entry/stop/target), committed on-chain before the open",
                    near::REPORT_SERVICE => "Axiom: full cited bull/bear research report for one symbol and session",
                    near::SEC_SERVICE => "Recent SEC filings for a ticker with 8-K red-flag item codes",
                    meme::MEME_SIGNALS_SERVICE => "Axiom: active NEAR memecoin breakout signals, each committed on-chain before sale",
                    meme::MEME_RISK_SERVICE => "Axiom: live rug-risk screen of one NEAR token (liquidity, age, holders, churn, copycats)",
                    _ => "",
                }
                .to_string()
            });
            let args = json!({"service_id": service, "price": parse_near(&price)?.to_string(), "description": description});
            let s = signer()?;
            let deposit = near.storage_deposit(&s, parse_near("0.01")?).await?;
            let r = near.call(&s, &contract()?, "register_service", args, deposit).await?;
            done(&format!("Registered {service} at {price} NEAR/call"), &r.hash);
        }
        NearCmd::Approve { service } => {
            let c = contract()?;
            let svc = near.view(&c, "get_service", json!({"service_id": service})).await?;
            if svc.is_null() {
                bail!("no service {service} on {c}");
            }
            println!("{service}: {} per call, seller {}", fmt_near(u128_field(&svc, "price")?), svc["provider"]);
            let r = near.call(&signer()?, &c, "approve_service", json!({"service_id": service, "price": svc["price"]}), 0).await?;
            done("Approved", &r.hash);
        }
        NearCmd::Revoke { service } => {
            let r = near.call(&signer()?, &contract()?, "revoke_service", json!({"service_id": service}), 0).await?;
            done("Revoked", &r.hash);
        }
        NearCmd::Receipts { last } => {
            let c = contract()?;
            let total = near.view(&c, "receipt_count", json!({})).await?.as_u64().unwrap_or(0);
            let rs = near.view(&c, "get_receipts", json!({"from": total.saturating_sub(last), "limit": last})).await?;
            for r in rs.as_array().into_iter().flatten() {
                let t = chrono::DateTime::<Utc>::from_timestamp((r["timestamp_ns"].as_u64().unwrap_or(0) / 1_000_000_000) as i64, 0).unwrap_or_default();
                let h = r["request_hash"].as_str().unwrap_or("");
                println!(
                    "#{:<4} {} {:<14} {:>10}  {} -> {}  {}  req {}…",
                    r["id"],
                    t.format("%Y-%m-%d %H:%M"),
                    r["service_id"].as_str().unwrap_or("?"),
                    fmt_near(u128_field(r, "amount")?),
                    r["payer"].as_str().unwrap_or("?"),
                    r["provider"].as_str().unwrap_or("?"),
                    r["memo"].as_str().unwrap_or(""),
                    &h[..h.len().min(12)]
                );
            }
            if total == 0 {
                println!("No receipts yet.");
            }
        }
        NearCmd::Serve { port, bids, endpoint } => {
            let ua = std::env::var("SEC_USER_AGENT").ok().filter(|s| !s.trim().is_empty());
            let bidding = if bids.is_empty() {
                None
            } else {
                let prices = parse_prices(&bids)?;
                let endpoint = endpoint.unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
                Some((signer()?, near::provider::BidPolicy { prices, endpoint }))
            };
            near::provider::serve(near::provider::SellerConfig {
                port,
                seller: me()?,
                contract: contract()?,
                db_path: db_path(),
                sec_user_agent: ua,
                sec: None,
                bidding,
                tag: String::new(),
            })
            .await?;
        }
        NearCmd::AgentCreate { name, role, budget, max_per_call, daily_cap, gas, storage } => {
            use near::client::FnAccess;
            let owner = signer()?;
            let c = contract()?;
            if name.contains('.') || name.is_empty() {
                bail!("name is the first part only; the agent will be <name>.{}", owner.account_id);
            }
            let agent_id = format!("{name}.{}", owner.account_id);
            if near.balance(&agent_id).await?.is_some() {
                bail!("{agent_id} already exists");
            }
            let mut methods: Vec<&str> = Vec::new();
            if role != "seller" {
                methods.extend(["pay", "post_job", "award", "confirm", "cancel_job", "open_pool", "join_pool", "refund_pool"]);
            }
            if role != "buyer" {
                methods.extend(["commit", "register_service", "bid", "claim"]);
            }
            let gas_amount = parse_near(&gas)?;
            let agent = Signer::generate(&agent_id)?;
            let r = near
                .send(
                    &owner,
                    &agent_id,
                    vec![
                        Action::CreateAccount,
                        Action::Transfer(gas_amount),
                        Action::AddKey {
                            public_key: agent.public_key(),
                            access: Some(FnAccess {
                                allowance: Some(gas_amount),
                                receiver: c.clone(),
                                methods: methods.iter().map(|m| m.to_string()).collect(),
                            }),
                        },
                        Action::AddKey { public_key: owner.public_key(), access: None },
                    ],
                )
                .await?;
            let path = agent.save(&near.network)?;
            done(&format!("Created {agent_id} ({role}); its key can only call {} on {c}", methods.join(", ")), &r.hash);
            println!("  agent key saved to {}; your key {} is its recovery key", path.display(), owner.public_key_str());
            let r = near.call(&owner, &c, "fund_storage", json!({"account": agent_id}), parse_near(&storage)?).await?;
            done(&format!("Prepaid {storage} NEAR storage credit for {agent_id}"), &r.hash);
            if role != "seller" {
                let wallet = near.view(&c, "get_wallet", json!({"owner": owner.account_id})).await?;
                if wallet.is_null() {
                    let budget = budget.as_deref().context("you have no wallet yet: pass --budget to open one")?;
                    let args = json!({"agent": agent_id, "max_per_call": parse_near(&max_per_call)?.to_string(), "daily_cap": parse_near(&daily_cap)?.to_string()});
                    let r = near.call(&owner, &c, "open_wallet", args, parse_near(budget)?).await?;
                    done(&format!("Opened your wallet ({budget} NEAR) with {agent_id} as its agent"), &r.hash);
                } else {
                    let r = near.call(&owner, &c, "add_agent", json!({"agent": agent_id}), parse_near("0.01")?).await?;
                    done(&format!("{agent_id} now spends from your wallet (shared budget and caps)"), &r.hash);
                    if let Some(b) = budget {
                        let r = near.call(&owner, &c, "deposit", json!({}), parse_near(&b)?).await?;
                        done(&format!("Added {b} NEAR to your budget"), &r.hash);
                    }
                }
                println!("  approve the services it may buy with `axiom near approve <service> --as {}`", owner.account_id);
            }
        }
        NearCmd::FundStorage { account, amount } => {
            let r = near.call(&signer()?, &contract()?, "fund_storage", json!({"account": account}), parse_near(&amount)?).await?;
            let total: u128 = r.value.as_ref().and_then(|v| v.as_str()).unwrap_or("0").parse()?;
            done(&format!("{account} storage credit is now {}", fmt_near(total)), &r.hash);
        }
        NearCmd::Agents => {
            let c = contract()?;
            let owner = me()?;
            let agents = near.view(&c, "get_agents", json!({"owner": owner})).await?;
            let list: Vec<String> = agents.as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
            if list.is_empty() {
                println!("{owner} has no wallet on {c}.");
            }
            for a in list {
                let bal = near.balance(&a).await?.map(fmt_near).unwrap_or_else(|| "no account".into());
                let credit = near.view(&c, "get_storage_credit", json!({"account": a})).await?;
                let credit = credit.as_str().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
                println!("  {a:<45} balance {bal:>14}  storage credit {}", fmt_near(credit));
            }
        }
        NearCmd::AgentSweep { agent, keep } => {
            let owner = signer()?;
            if !agent.ends_with(&format!(".{}", owner.account_id)) {
                bail!("{agent} is not a sub-account of {}", owner.account_id);
            }
            let bal = near.balance(&agent).await?.context("no such account")?;
            let amount = bal.saturating_sub(parse_near(&keep)?);
            if amount == 0 {
                bail!("{agent} has only {}", fmt_near(bal));
            }
            let r = near.send(&owner.acting_as(&agent), &owner.account_id, vec![Action::Transfer(amount)]).await?;
            done(&format!("Moved {} from {agent} to {}", fmt_near(amount), owner.account_id), &r.hash);
        }
        NearCmd::Hire { topic, max_price, wait } => {
            let ctx = near::Ctx::from_env(as_account)?;
            let h = near::swarm::hire(&ctx, &topic, parse_near(&max_price)?, std::time::Duration::from_secs(wait), usize::MAX).await?;
            println!("{}", serde_json::to_string_pretty(&h.result)?);
        }
        NearCmd::Jobs { last } => {
            let c = contract()?;
            let n = near.view(&c, "job_count", json!({})).await?.as_u64().unwrap_or(0);
            let jobs = near.view(&c, "get_jobs", json!({"from": n.saturating_sub(last), "limit": last})).await?;
            for j in jobs.as_array().into_iter().flatten().rev() {
                println!(
                    "#{:<4} {:<10} {:<34} max {:>10}  buyer {}  winner {} at {}",
                    j["id"],
                    j["status"].as_str().unwrap_or("?"),
                    j["topic"].as_str().unwrap_or(""),
                    fmt_near(u128_field(j, "max_price")?),
                    j["buyer"].as_str().unwrap_or("?"),
                    j["winner"].as_str().unwrap_or("-"),
                    fmt_near(u128_field(j, "price")?)
                );
                for b in near.view(&c, "get_bids", json!({"job_id": j["id"]})).await?.as_array().into_iter().flatten() {
                    println!("        bid {:>10} {}", fmt_near(u128_field(b, "price")?), b["seller"].as_str().unwrap_or("?"));
                }
            }
            if n == 0 {
                println!("No jobs yet.");
            }
        }
        NearCmd::Pools { last } => {
            let c = contract()?;
            let n = near.view(&c, "pool_count", json!({})).await?.as_u64().unwrap_or(0);
            let pools = near.view(&c, "get_pools", json!({"from": n.saturating_sub(last), "limit": last})).await?;
            for p in pools.as_array().into_iter().flatten().rev() {
                let members: Vec<&str> = p["members"].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str).collect();
                println!(
                    "#{:<4} {:<9} {:<14} {:>10} split {}/{}  receipt {}  {}",
                    p["id"],
                    p["status"].as_str().unwrap_or("?"),
                    p["service_id"].as_str().unwrap_or("?"),
                    fmt_near(u128_field(p, "price")?),
                    members.len(),
                    p["shares"],
                    p["receipt"].as_u64().map(|r| format!("#{r}")).unwrap_or_else(|| "-".into()),
                    members.join(", ")
                );
            }
            if n == 0 {
                println!("No pools yet.");
            }
        }
        NearCmd::Swarm { sellers, buyers, symbols, days, max_price, pool_seller, pool_date, base_port } => {
            let since = (Utc::now().date_naive() - TimeDelta::days(days)).to_string();
            let mut specs = Vec::new();
            for (i, s) in sellers.iter().enumerate() {
                let (account, price) = s.split_once('=').context("--seller takes ACCOUNT=NEAR")?;
                specs.push(near::swarm::SellerSpec {
                    account: account.to_string(),
                    port: base_port + i as u16,
                    prices: near::swarm::sec_prices(parse_near(price)?),
                });
            }
            let pool = match (pool_seller, pool_date) {
                (Some(url), Some(d)) => Some((url, near::SIGNALS_SERVICE.to_string(), vec![("date", d.to_string())], vec![d.to_string()])),
                (None, None) => None,
                _ => bail!("--pool-seller and --pool-date go together"),
            };
            let plan = near::swarm::SwarmPlan {
                sellers: specs,
                buyers,
                topics: symbols.iter().map(|s| format!("{}|{}|{since}", near::SEC_SERVICE, s.to_uppercase())).collect(),
                max_price: parse_near(&max_price)?,
                pool,
                db_path: db_path(),
                sec_user_agent: std::env::var("SEC_USER_AGENT").ok().filter(|s| !s.trim().is_empty()),
            };
            near::swarm::run(plan, contract()?).await?;
        }
        NearCmd::Buy { symbol, provider, days } => {
            let ctx = near::Ctx::from_env(as_account)?;
            let since = Utc::now().date_naive() - TimeDelta::days(days);
            let filings = near::buy_filings(&ctx, &provider, &symbol.to_uppercase(), since).await?;
            println!("{} filing(s) for {} since {since}:", filings.len(), symbol.to_uppercase());
            for f in &filings {
                let red = f.items.split(',').any(|c| sec::is_red_flag(c.trim()));
                println!("  {:<6} {} {:<12}{} {}", f.form, f.filed, f.items, if red { " RED FLAG" } else { "" }, f.url);
            }
        }
        NearCmd::CheckSeller { seller } => {
            check_seller(&near, &seller).await?;
        }
        NearCmd::Shop { seller, date, min_verified, max_unverified_share, reports } => {
            let ctx = near::Ctx::from_env(as_account)?;
            let (info, v) = check_seller(&near, &seller).await?;
            if info["contract"].as_str() != Some(ctx.contract.as_str()) {
                bail!("seller settles on {}, our wallet is on {}", info["contract"], ctx.contract);
            }
            // The buying decision: only verified, on-time history counts.
            let on_time = v.count(near::Check::OnTime);
            let unverified = v.rows.len() - on_time;
            let share = if v.rows.is_empty() { 0.0 } else { unverified as f64 / v.rows.len() as f64 };
            println!();
            if on_time < min_verified {
                bail!("DECISION: not buying. Only {on_time} signal(s) verified on-chain; policy requires {min_verified}.");
            }
            if share > max_unverified_share {
                bail!("DECISION: not buying. {:.0}% of published signals are late or uncommitted; policy allows {:.0}%.", share * 100.0, max_unverified_share * 100.0);
            }
            let date = match date.or_else(|| info["latest_signal_date"].as_str().and_then(|d| d.parse().ok())) {
                Some(d) => d,
                None => bail!("seller has no signals to sell"),
            };
            println!("DECISION: buying. {on_time} signal(s) verified before entry, {unverified} unverified. Buying signals for {date}.");
            let d = date.to_string();
            let bought = near::purchase(&ctx, &seller, near::SIGNALS_SERVICE, &[("date", d.clone())], &[&d]).await?;
            let sigs = bought["signals"].as_array().cloned().unwrap_or_default();
            println!("\n{} signal(s) for {date}:", sigs.len());
            for s in &sigs {
                // A fresh signal should already be committed by the seller: check before trusting it.
                let committed = !near
                    .view(&ctx.contract, "find_commitment", json!({"committer": info["seller"], "hash": s["commitment_hash"]}))
                    .await?
                    .is_null();
                println!(
                    "  {:<6} {:<16} entry ~{:.2} stop {:.2} target {:.2} R:R {:.1}  {}",
                    s["symbol"].as_str().unwrap_or("?"),
                    s["setup"].as_str().unwrap_or("?"),
                    s["entry"].as_f64().unwrap_or(0.0),
                    s["stop"].as_f64().unwrap_or(0.0),
                    s["target"].as_f64().unwrap_or(0.0),
                    s["risk_reward"].as_f64().unwrap_or(0.0),
                    if committed { "committed on-chain" } else { "NOT committed: treat as unverified" }
                );
            }
            if reports {
                let dir = std::path::Path::new("purchased").join(&d);
                std::fs::create_dir_all(&dir)?;
                for s in &sigs {
                    let sym = s["symbol"].as_str().unwrap_or_default().to_string();
                    let r = near.view(&ctx.contract, "get_service", json!({"service_id": near::REPORT_SERVICE})).await?;
                    if r.is_null() {
                        println!("  (seller does not sell reports)");
                        break;
                    }
                    let rep = near::purchase(&ctx, &seller, near::REPORT_SERVICE, &[("symbol", sym.clone()), ("date", d.clone())], &[&sym, &d]).await?;
                    for (i, r) in rep["reports"].as_array().into_iter().flatten().enumerate() {
                        let path = dir.join(format!("{sym}-{}.md", r["setup"].as_str().unwrap_or(&i.to_string())));
                        std::fs::write(&path, r["markdown"].as_str().unwrap_or_default())?;
                        println!("  report saved: {}", path.display());
                    }
                }
            }
        }
        NearCmd::Claim { job_id } => {
            let ctx = near::Ctx::from_env(as_account)?;
            let r = ctx.near.call(&ctx.signer, &ctx.contract, "claim", json!({"job_id": job_id}), 0).await?;
            done(&format!("{} claimed the escrowed payment for job #{job_id}", ctx.signer.account_id), &r.hash);
        }
        NearCmd::CommitSignals => {
            let ctx = near::Ctx::from_env(as_account)?;
            let store = Store::open(&db_path())?;
            let mut n = 0;
            for s in store.all()? {
                let hash = near::stored_hash(&s);
                let found = ctx.near.view(&ctx.contract, "find_commitment", json!({"committer": ctx.signer.account_id, "hash": hash})).await?;
                if !found.is_null() {
                    continue;
                }
                let tx = near::commit_signal(&ctx, &hash, &format!("axiom-signal/v1 {}", s.as_of)).await?;
                println!("{} {} {}: {}", s.symbol, s.setup, s.as_of, ctx.near.explorer_tx(&tx));
                n += 1;
            }
            println!("Committed {n} signal(s) as {}.", ctx.signer.account_id);
        }
        NearCmd::ExportRecord { out } => {
            let seller = me()?;
            let now = Utc::now();
            let all = Store::open(&db_path())?.all()?;
            let entries: Vec<near::RecordEntry> =
                all.iter().filter(|s| near::entry_known_at(s.as_of, s.fill_date) <= now).map(near::RecordEntry::from).collect();
            let body = json!({
                "seller": seller,
                "committer": seller,
                "contract": contract()?,
                "network": near.network,
                "generated_at": now,
                "entries": entries,
            });
            if let Some(dir) = std::path::Path::new(&out).parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&out, serde_json::to_string_pretty(&body)?)?;
            println!("Wrote {} published signal(s) to {out} ({} still fresh, not published).", entries.len(), all.len() - entries.len());
        }
        NearCmd::Verify => {
            let store = Store::open(&db_path())?;
            let entries: Vec<near::RecordEntry> = store.all()?.iter().map(near::RecordEntry::from).collect();
            if entries.is_empty() {
                println!("No signals in the ledger yet. Run `axiom scan --near` first.");
                return Ok(());
            }
            let v = near::verify_entries(&near, &contract()?, &me()?, entries).await?;
            near::print_verified(&v);
        }
    }
    Ok(())
}

/// Fetches a seller's info and free track record, and verifies every entry against its on-chain commitments.
async fn check_seller(near: &near::client::Near, url: &str) -> Result<(serde_json::Value, near::Verified)> {
    let http = reqwest::Client::new();
    let base = url.trim_end_matches('/');
    let info: serde_json::Value = http.get(format!("{base}/")).send().await.with_context(|| format!("seller {base}"))?.json().await?;
    let record: serde_json::Value = http.get(format!("{base}/v1/track-record")).send().await?.json().await?;
    let seller = info["seller"].as_str().context("seller info has no account")?;
    let contract = info["contract"].as_str().context("seller info has no contract")?;
    if record["committer"].as_str() != Some(seller) {
        bail!("track record is committed by {}, not the seller {seller}", record["committer"]);
    }
    let entries: Vec<near::RecordEntry> = serde_json::from_value(record["entries"].clone()).context("bad track record")?;
    println!("Seller {seller} on {contract}: {} published signal(s). Checking each on-chain...\n", entries.len());
    let v = near::verify_entries(near, contract, seller, entries).await?;
    near::print_verified(&v);
    Ok((info, v))
}
