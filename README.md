# Axiom

A personal research agent for US stocks, written in Rust. It scans a watchlist after each market close and finds a small set of **named, backtestable setups**. For each hit it gathers evidence (technicals, market regime, SEC filings, news) and writes a **cited bull/bear research report**. Every signal is logged as a **paper trade**, so you can measure whether the agent is any good.

**Research only.** It never connects to a brokerage and never places trades.

> **NEAR HACKATHON:** with Axiom Pay, Axiom **sells its research to other AI agents** on NEAR, pay-per-call. Every signal is committed on-chain before the next market open, so a buyer agent can verify Axiom's track record *before* paying, and its human owner caps what it can spend. Axiom also buys its own evidence from other agents the same way. It researches **NEAR memecoins** too (rug-risk screens and breakout signals whose outcomes buyers recompute from public prices), gives each agent a **restricted wallet of its own**, and runs an **agent swarm** where sellers bid for jobs and buyers pool purchases. Design, demo steps, live testnet deployment and limits: **[NEAR.md](NEAR.md)**.

## How it works

```
axiom scan
  │
  ├─ Alpaca      → adjusted daily bars (watchlist + SPY), consolidated SIP tape
  ├─ indicators  → SMA/EMA/RSI/ATR/volume, deterministic, unit-tested
  ├─ setups      → rule-based signal on the latest close (entry / stop / target from ATR)
  ├─ backtest    → this setup's track record on this symbol and on the whole watchlist, vs SPY
  ├─ SEC EDGAR   → 8-K/10-Q/10-K/Form 4/dilution filings in the last 30 days (red flags marked)
  ├─ Alpaca News → headlines from the last 7 days
  ├─ free LLM    → bull case, bear case, invalidation, verdict, citing evidence IDs only
  ├─ reports/    → Markdown report per signal
  ├─ axiom.db    → SQLite ledger (paper trades)
  └─ Telegram    → short alert
```

Design rules:
- **Code computes, the LLM explains.** Prices, levels, indicators and statistics all come from deterministic Rust. The LLM only interprets evidence it is given, and cites it as `[E3]`.
- **There is no "proof" a trade will work.** What you get instead: the setup's historical record net of costs, its **edge over simply holding SPY**, an in-sample / out-of-sample split, and a live paper-trade scorecard.
- **The bear case is mandatory.** Opposing evidence (weak volume, overextension, a market below its 200-day average, 8-K red flags, dilution filings) is gathered by code. The model is told to argue against the trade.
- **Scraped text is untrusted.** Headlines are passed to the model as data, with explicit instructions to ignore any instructions inside them.

## Setup

1. Install Rust (`rustup`). You already have it.
2. Get free keys:
   - **Alpaca**: sign up, create a paper account, generate API keys (data works on the free Basic plan).
   - **SEC**: no key; just set `SEC_USER_AGENT` to `"<app> <your email>"`.
   - **Free LLM** (optional): [Ollama](https://ollama.com) locally (`ollama pull qwen3:8b`, no key), or a free Groq / Gemini key. Any OpenAI-compatible endpoint works; see `.env.example`.
   - **Telegram** (optional): create a bot with @BotFather; get your chat ID from @userinfobot.
3. Run `cp .env.example .env` and fill it in.
4. Edit `config/watchlist.txt`.

## Usage

```bash
# 0. Get an unbiased universe (today's S&P 500 members) -> config/sp500.txt
cargo run --release -- universe sp500

# 1. First, check whether the setups have any edge at all (no LLM cost)
cargo run --release -- backtest --watchlist config/sp500.txt
cargo run --release -- backtest --watchlist config/sp500.txt --setup breakout-52w --csv trades.csv

# 2. Daily, after the 4pm ET close (e.g. via cron / launchd)
cargo run --release -- scan
cargo run --release -- scan --symbols NVDA,AMD --no-llm --no-telegram

# Today's ranking for a monthly portfolio (momentum, quality, value, quality-value)
cargo run --release -- rank --by quality-value

# Earnings surprises filed in the last 14 days
cargo run --release -- earnings

# 3. Score the paper trades
cargo run --release -- track

cargo test
```

A run of `scan` is idempotent: a signal already logged for that symbol/setup/date is skipped, so re-running doesn't re-bill the LLM.

**Price cache:** all bars are stored in SQLite (`AXIOM_DB`). The first run downloads full history (S&P 500 × 8 years ≈ 1M bars). It saves batch by batch, so an interrupted download resumes. Later runs fetch only the last few days. When a split or dividend changes past prices, Axiom detects it and re-downloads that symbol.

**How to read a backtest:** the line that matters is `vs same stocks, random day`. Beating SPY alone can just mean the stock list was picked with hindsight. Only a setup whose 90% confidence interval clears the random-entry baseline has a timing edge.

### Scheduling (macOS)
```cron
# crontab -e   (times are local; 22:30 WAT = 4:30pm ET in winter, adjust for DST)
30 22 * * 1-5 cd ~/Documents/GitHub/axiom && ./target/release/axiom scan >> axiom.log 2>&1 && ./target/release/axiom track >> axiom.log 2>&1
```

## Setups

| Setup | Idea | Stop / target | Max hold |
|---|---|---|---|
| `trend-pullback` | Uptrend (close > SMA200, SMA50 > SMA200), pullback holds the 21-day EMA, RSI 35–55, bullish close | 2×ATR / 4×ATR | 20 sessions |
| `breakout-52w` | Close above the prior 52-week high on ≥1.5× average volume, above SMA50 | 2×ATR / 4×ATR | 40 sessions |
| `rsi2-pullback` | Short-term reversal: close > SMA200, RSI(2) < 10. Exit on the first close above SMA5 | 3×ATR catastrophe stop / 3×ATR cap | 10 sessions |
| `gap-drift` | Post-announcement drift proxy: gap up ≥4% on ≥3× volume that holds to the close | stop at the prior close / 8×ATR cap | 40 sessions |
| `momentum-12-1` | Portfolio: each month-end hold the top 10% by 12-month return (skipping the last month), equal-weight | none, monthly rebalance | 1 month |
| `quality` | Portfolio: top 10% by trailing-12-month operating cash flow / total assets | none, monthly rebalance | 1 month |
| `value` | Portfolio: top 10% by earnings yield (TTM diluted EPS / price as quoted then) | none, monthly rebalance | 1 month |
| `quality-value` | Portfolio: top 10% by the average of the quality and value percentile ranks | none, monthly rebalance | 1 month |
| `earnings-drift` | Event: strong earnings beat (SUE ≥ 2, from the company's own reported EPS), bought after the 10-Q/10-K filing | none | 60 sessions |

**Fundamentals are point-in-time.** Every SEC value keeps its filing date. A backtest on date D only uses numbers filed by D; when a number was later restated, it uses the version known on D. The value ratio uses unadjusted monthly prices, because the cached daily prices are split-adjusted after the fact and would distort P/E.

S&P 500 result (2018–2026) for v1: `trend-pullback` and `breakout-52w` were both **significantly worse than random entry**, in and out of sample. They are kept as references, not as tradeable ideas.
The backtest's `entries only` line holds each signal for a fixed period with no exits. It shows whether a setup's weakness is in its entries or in its stop/target rules.

To add a setup, implement `Setup` in [src/setups.rs](src/setups.rs), register it in `all()`, and run `backtest`.
**Only change the rules while looking at the in-sample numbers.** Out-of-sample is your honest estimate; tuning against it turns it into in-sample.

## Known limitations (read before trusting anything)

- **Survivorship bias:** backtests use today's index members. A stock only counts from the date it joined the S&P 500 (from `universe sp500`), which removes the "added because it already went up" hindsight. Stocks *removed* from the index since then are still missing, so results remain somewhat optimistic.
- **Daily bars only:** when a stop and a target fall inside the same bar, the stop is assumed hit first (conservative). Intraday trading is out of scope on free data.
- **News and filings are context, not scored signals:** there is no point-in-time news archive in the backtest, so news can't be validated historically. The model interprets it, and its interpretation is labelled as such.
- **Form 4 direction is not parsed yet** (only counted). The same goes for filing contents: only form type and 8-K item codes are read.
- **No earnings calendar yet:** a swing trade held into earnings carries gap risk that the report can't see.
- **No fundamentals yet**, so the "long-term" horizon isn't covered. See the roadmap.

## Roadmap

1. ✅ MVP: scan → evidence → cited report → paper ledger → Telegram
2. ✅ Fundamentals from SEC XBRL (point-in-time by filing date): earnings surprise, quality, value.
3. Earnings calendar (Finnhub free) → flag or block swing entries that would be held through earnings.
4. Form 4 parsing (insider buys vs sells, size); 8-K body text extraction for the model.
5. Confidence calibration from the paper ledger once there are 50+ closed trades per setup.
6. Wider universe: S&P 500 + Nasdaq-100 daily scan.
7. Web app and personal trade journal (details below).

### Planned: web app and trade journal

The goal: Axiom follows the trades **you actually take**, not just its own paper trades.

**Capital and position sizing**
- You enter the amount you want to invest (account size), plus a risk-per-trade setting (e.g. 1%).
- Each suggestion then shows a concrete size: `shares = (account × risk%) ÷ (entry − stop)`, capped so a single position can't take up too much of the account.

**On every suggested trade**
- **"I took it"**: records a position at the suggested entry, stop, target and size. An optional "actual fill" field covers slippage.
- **"I passed"**: records the skip, so you can later compare the trades you took with the ones you passed on.
- **"I closed it"**: exit price, date and reason (stop / target / manual / other).

**Position monitoring and email alerts** (only for trades marked "taken")
- Stop hit or target hit.
- **Thesis broken:** a new red-flag 8-K or dilution filing, a close below the setup's key level (e.g. the 21-day EMA or the breakout level), or the market regime turning risk-off.
- **Time:** approaching the max holding period, or earnings coming up while the position is still open.
- **Scale-in ("buy more"):** only when a defined, backtested add-on rule fires, never on the LLM's judgment alone.
- A daily or weekly summary of open positions, with unrealized P&L and distance to the stop and the target.
- Emails are sent by the app itself (SMTP or the Gmail API).

**User history the AI can read**
- Everything is stored per user: capital, taken/passed/closed trades, actual fills, exits and reasons.
- The analyst model gets a summary of your record when writing a new report. Examples: your win rate per setup, whether you exit early, which setups you skip, and how the trades you took compare with the system's paper trades.
- Two separate ledgers: **paper trades** measure the system; the **journal** measures you.

**Stack:** Axum REST API and Postgres (moving off SQLite) plus a web UI. Login and accounts come later; it starts as a single user.
