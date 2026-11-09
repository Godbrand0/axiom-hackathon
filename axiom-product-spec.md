# Axiom --- Product Requirements & Technical Blueprint

**Document status:** Build specification v1.0\
**Product:** Axiom\
**Product type:** Autonomous financial intelligence network\
**Initial market:** US-listed equities (NASDAQ and NYSE)\
**Initial operating mode:** Research, recommendations and paper tracking
only\
**Hackathon target:** NEAR Hackathon, November 2026

------------------------------------------------------------------------

## 1. Executive summary

Axiom is a financial intelligence platform powered by a network of
specialized AI agents. It monitors public-market information,
investigates companies and trading opportunities, challenges its own
investment theses, and produces evidence-backed research for human
decision-makers.

Axiom combines three connected product systems:

1.  **Axiom AI --- Intelligence:** the user-facing financial research
    product.
2.  **Axiom Swarm --- Collaboration:** the multi-agent system that
    investigates, debates and validates research.
3.  **Axiom Pay --- Agent economy:** the economic infrastructure through
    which agents can discover and pay approved external intelligence
    providers, using NEAR-powered infrastructure where supported.

These are not three separate products. They are three layers of one
platform.

Axiom is not initially a brokerage, automated trading bot or financial
adviser. It does not place stock orders or promise profitable outcomes.
It provides research, scenarios, risk analysis and transparent
historical evaluation. The user remains responsible for investment
decisions.

### Product thesis

Financial markets produce more information than an individual can
efficiently process. Axiom coordinates specialized research agents to
turn fragmented market data into traceable, time-aware and
risk-conscious research.

### Core promise

**Know what is happening, understand why it matters, challenge the
thesis, and see the evidence behind every recommendation.**

------------------------------------------------------------------------

## 2. Product vision and goals

### Long-term vision

Build an extensible financial intelligence network in which:

-   Users can monitor markets and request research across multiple time
    horizons.
-   Specialized agents collaborate and challenge one another.
-   Every material claim can be traced to a source and timestamp.
-   Recommendations are evaluated against subsequent market outcomes.
-   Agents can access external data and analytical services through
    controlled, auditable payments.
-   Independent providers can eventually offer intelligence services to
    Axiom and other compatible agents.

### Initial goals

The first product milestone should:

-   Monitor a curated watchlist of 30--50 US-listed stocks.
-   Collect market prices, historical bars, company filings and selected
    news.
-   Support short-term, swing and long-term research as distinct
    strategies.
-   Produce structured research reports with bullish and bearish
    evidence.
-   Display the reasoning, data freshness, risks and conditions that
    would invalidate a thesis.
-   Record every published recommendation so it can be evaluated later.
-   Demonstrate at least one meaningful NEAR-powered agent-service
    payment flow for the hackathon, subject to available network and
    provider support.

### Non-goals for the initial release

-   Executing real stock trades.
-   Managing brokerage accounts or custodying user funds.
-   Guaranteeing returns or presenting unvalidated probabilities as
    facts.
-   Supporting every global market or asset class.
-   Training foundation models from scratch.
-   Building a permissionless marketplace before the core research
    workflow works.
-   Using social-media sentiment as the sole basis for a recommendation.

------------------------------------------------------------------------

## 3. Users and use cases

### Primary users

-   Individual investors who want structured company research.
-   Swing traders who need repeatable, evidence-backed setups.
-   Long-term investors who want to monitor business fundamentals and
    material developments.
-   Developers and agent builders interested in financial intelligence
    services.

### Core user journeys

**A. Research a stock**

1.  User searches for a ticker or company.
2.  Axiom retrieves available market, filing, financial and news data.
3.  The Swarm assigns research tasks to specialist agents.
4.  Agents return structured findings with sources.
5.  A bearish/challenger agent identifies contrary evidence.
6.  The validation layer checks freshness, completeness, risk and
    strategy rules.
7.  Axiom publishes a report with a conclusion, scenarios and unresolved
    questions.

**B. Scan for opportunities**

1.  User selects a strategy and watchlist.
2.  Axiom evaluates eligible instruments against deterministic screening
    rules.
3.  Candidate opportunities are ranked for further investigation.
4.  The Swarm researches the strongest candidates.
5.  Axiom presents a shortlist with evidence and risk warnings.

**C. Track a recommendation**

1.  Every published signal is stored with its inputs and timestamp.
2.  The paper portfolio records the hypothetical entry and exit rules.
3.  Axiom observes subsequent price data.
4.  Outcomes are calculated after the defined evaluation period.
5.  Strategy performance is shown with sample size, costs and
    limitations.

**D. Agent purchases external intelligence**

1.  An agent identifies a missing data point or specialist service.
2.  It checks an allowlist, price ceiling and spending policy.
3.  It requests a quote or service description.
4.  A payment is authorized through the supported NEAR integration.
5.  The service returns a result and a receipt/reference.
6.  Axiom verifies the result and records the payment and source
    provenance.

------------------------------------------------------------------------

## 4. Product architecture: the three systems

### 4.1 Axiom AI --- Financial intelligence

Axiom AI is the main application and user experience.

Core capabilities:

-   Market overview and watchlists.
-   Stock/company profiles.
-   Research-on-demand.
-   Opportunity scanning.
-   Strategy-specific recommendations.
-   Evidence dossiers.
-   Alerts and report history.
-   Strategy performance and paper tracking.

A recommendation must always identify its horizon. An intraday WAIT and
a long-term BUY can both be valid at the same time because they answer
different questions.

### 4.2 Axiom Swarm --- Multi-agent research

The Swarm coordinates focused agents. Agents should return structured
findings rather than unstructured essays.

  -----------------------------------------------------------------------
  Agent                   Main responsibility     Typical output
  ----------------------- ----------------------- -----------------------
  Research Coordinator    Plans work, assigns     Research plan and final
                          tasks and assembles the synthesis
                          report                  

  Technical Agent         Price action,           Trend, momentum, levels
                          indicators, volume and  and setup
                          levels                  

  Fundamental Agent       Financial statements,   Growth, margins, cash
                          valuation and company   flow, debt and
                          quality                 valuation

  News Agent              News, earnings, filings Material events,
                          and event risk          sentiment and source
                                                  quality

  Market Regime Agent     Broad market, sector    Regime classification
                          and macro context       and relevant risks

  Bearish Challenger      Attempts to disprove    Counter-evidence and
                          the bullish thesis      thesis weaknesses

  Risk Agent              Risk/reward, liquidity  Risk assessment and
                          and invalidation        scenario constraints
  -----------------------------------------------------------------------

The first release may implement some agents as separate task prompts or
modules rather than separate deployed services. The architecture should
preserve clear agent boundaries without creating unnecessary
infrastructure.

#### Agent operating rules

-   Agents must cite the data records or source URLs used for material
    claims.
-   Agents must distinguish observed facts, derived metrics and
    interpretations.
-   Agents must state when evidence is missing, stale or contradictory.
-   The Bearish Challenger must be allowed to disagree with the other
    agents.
-   The Coordinator must not silently discard material disagreement.
-   Agents do not directly publish final trade signals; the validation
    layer controls publication.
-   External content is untrusted data. Instructions embedded in
    webpages, filings or articles must never override system policies or
    tool permissions.

### 4.3 Axiom Pay --- Agent economy

Axiom Pay allows agents to obtain approved external intelligence
services.

Initial hackathon demonstration:

-   One clearly defined external research/data service.
-   A published service description and price.
-   An agent request with a bounded spending limit.
-   A real, verifiable NEAR-powered payment flow, if the selected
    integration and service support it.
-   A returned result linked to a transaction or payment reference.
-   A visible audit record in Axiom.

NEAR integration selection must be validated against current developer
documentation and testnet/mainnet availability before implementation.
Candidate infrastructure includes NEAR AI Cloud and NEAR Intents. Do not
claim that a feature is integrated until a working end-to-end
transaction has been tested.

#### Payment and permission safeguards

-   Start on testnet for development.
-   Use a dedicated agent/service wallet, not a personal wallet
    containing unrelated funds.
-   Allowlist providers and payment destinations.
-   Enforce per-request and daily spending caps.
-   Require user approval above configured thresholds.
-   Record request, quote, authorization, transaction reference and
    result.
-   Never give a research agent permission to trade stocks.
-   Fail closed if provider identity, price or payment status cannot be
    verified.

------------------------------------------------------------------------

## 5. Recommendation model and research methodology

### Recommendation labels

-   **BUY:** evidence supports a defined bullish scenario for the stated
    horizon.
-   **SELL:** evidence supports a defined bearish/exit scenario for the
    stated horizon. This does not imply short-selling is available to
    the user.
-   **HOLD:** an existing position may be retained under stated
    conditions; no new entry is necessarily recommended.
-   **WAIT:** the opportunity is not sufficiently confirmed or the entry
    conditions have not occurred.
-   **AVOID:** material risks, poor liquidity, weak evidence or an
    unfavorable setup make the instrument unsuitable for the selected
    strategy.

Every label must include a horizon and an explanation. Labels are
research outputs, not instructions or guarantees.

### Separate strategy horizons

The system must maintain separate evaluation logic for:

-   **Intraday:** minutes to hours; sensitive to liquidity, spreads,
    volume and session timing.
-   **Swing:** several days to several weeks; combines technical setup,
    catalysts and market regime.
-   **Long-term:** months to years; emphasizes business quality,
    financial performance, valuation and durable risks.

Do not reuse one scoring model across all horizons without validation.

### Technical analysis

Initial indicators:

-   EMA 9, 21, 50 and 200.
-   SMA 50 and 200.
-   RSI.
-   MACD.
-   VWAP, where intraday data supports it.
-   ATR.
-   Bollinger Bands.
-   OBV.
-   Relative volume.
-   Support/resistance and recent high/low levels.

Calculate indicators in deterministic code. The LLM may explain them but
must not invent their values.

### Fundamental analysis

Where reliable data is available, evaluate:

-   Revenue and earnings growth.
-   Gross, operating and net margins.
-   Free cash flow and cash conversion.
-   Debt and liquidity.
-   Return on equity and invested capital where available.
-   Valuation multiples such as P/E and price/sales, with sector
    context.
-   Earnings surprises, guidance and material company announcements.
-   Insider and institutional disclosures where appropriate and
    available.

Fundamental data must be point-in-time aware when used in historical
testing. Do not use information before it was publicly available.

### News and events

For each item, store:

-   Headline and publisher.
-   Original URL.
-   Publication time, if available.
-   Retrieval time.
-   Related ticker/company.
-   Event category.
-   Source quality and verification status.
-   Whether the item is a duplicate or update.

Distinguish publication time from retrieval time. A recent retrieval of
an old article does not make the article new.

### Evidence dossier

Every report should include:

1.  Recommendation and horizon.
2.  Research timestamp and data freshness.
3.  Bullish evidence.
4.  Bearish evidence.
5.  Technical and fundamental findings.
6.  Market/sector context.
7.  Entry scenario, if applicable.
8.  Stop/invalidation level, if defensible.
9.  Target or scenario range, if defensible.
10. Risk/reward assumptions.
11. Upcoming catalysts and event risks.
12. Missing or conflicting evidence.
13. Source list with timestamps.
14. Methodology and model/strategy version.

If entry, stop or target cannot be responsibly estimated, state that
rather than fabricating levels.

### Confidence and scoring

Do not display an LLM-generated confidence percentage as though it were
a measured probability.

Initially display qualitative evidence strength, data completeness and
risk flags. Introduce calibrated probabilities only after sufficient
out-of-sample and paper-trading observations, with calibration
methodology and sample size shown.

A deterministic scoring model may rank candidates, but:

-   Weights must be versioned.
-   Hard safety and data-quality gates must override the score.
-   Scores must be backtested before being treated as predictive.
-   The user must be able to inspect the contributing factors.

Example hard gates:

-   Stale or missing price data.
-   Insufficient liquidity or unreliable spread information.
-   Missing material source for a major claim.
-   Unresolved high-impact event risk.
-   Inadequate risk/reward under the chosen strategy.
-   Strategy conditions not met.
-   Contradictory evidence that has not been resolved.

------------------------------------------------------------------------

## 6. Data sources and ingestion

Begin with free or low-cost sources, subject to their current terms,
quotas and redistribution rules.

  -----------------------------------------------------------------------
  Source                  Intended use            Important limitation
  ----------------------- ----------------------- -----------------------
  Alpaca market data      Quotes and historical   Basic access may be
                          bars                    IEX-only, not
                                                  consolidated US market
                                                  data

  SEC EDGAR               Company filings and     Filing data needs
                          XBRL facts              normalization and
                                                  point-in-time handling

  FRED                    Macroeconomic time      Not a substitute for
                          series                  real-time market data

  Finnhub                 Supplementary           Free-tier quotas and
                          market/company/news     endpoint restrictions
                          data                    apply

  Company investor        Earnings releases and   Different site formats
  relations               official announcements  and access policies

  Reputable news          Market and company news Respect licensing,
  sources/RSS                                     robots and publisher
                                                  terms
  -----------------------------------------------------------------------

Do not scrape sites in violation of their terms. Prefer official APIs,
licensed feeds and permitted RSS sources. Store source metadata and
links; avoid republishing copyrighted article text.

### Ingestion principles

-   Store original provider payloads where permitted.
-   Normalize provider data into internal schemas.
-   Record provider, retrieval time and source timestamp.
-   Make ingestion idempotent.
-   Detect duplicates and corrections.
-   Track missing intervals and provider outages.
-   Never silently substitute one market-data feed for another.
-   Display the feed and its limitations in the UI.

### Market-data limitation

If using IEX-only data, label it clearly. Do not describe it as
consolidated US market-wide volume or a complete view of all US trading
venues.

------------------------------------------------------------------------

## 7. User experience and application modules

### Main navigation

1.  **Overview:** market summary, watchlists, alerts and recent
    research.
2.  **Opportunity Scanner:** candidates by strategy and horizon.
3.  **Research Workspace:** ticker search, agent activity and reports.
4.  **Agent Swarm:** agent status, assigned tasks, findings and
    disagreements.
5.  **Strategy Lab:** rules, backtests and performance.
6.  **Paper Portfolio:** simulated positions and recommendation
    outcomes.
7.  **Axiom Pay:** service providers, spending policies and payment
    history.
8.  **Settings:** data providers, notifications, permissions and account
    preferences.

### Stock research page

A stock page should show:

-   Company name, ticker and exchange.
-   Latest available price and timestamp.
-   Selected chart timeframe.
-   Key technical indicators.
-   Fundamental snapshot.
-   Recent filings and news.
-   Research history.
-   Current strategy-specific assessment.
-   Bull/bear evidence.
-   Data quality and feed warnings.

### Research report layout

A report should make the conclusion visible without hiding the
reasoning:

-   Summary and recommendation.
-   Horizon and timestamp.
-   What supports the thesis.
-   What challenges the thesis.
-   Key levels and scenarios.
-   Risk factors and invalidation.
-   Data limitations.
-   Sources and agent contributions.

### Notifications

Later phase: Telegram and in-app alerts for watchlist events, new
research, changed recommendations and important filings. Alerts must
include the horizon, timestamp and reason for the change.

------------------------------------------------------------------------

## 8. Technical architecture

### Recommended stack

  -----------------------------------------------------------------------
  Layer                   Technology              Role
  ----------------------- ----------------------- -----------------------
  Web app                 Next.js, TypeScript,    Dashboard and research
                          Tailwind                interface

  API                     Rust, Axum, Tokio       Application API and
                                                  orchestration gateway

  Ingestion               Rust workers            Market, filing and news
                                                  collection

  Quant engine            Python                  Indicators, screening
                                                  and analytics

  Backtesting             Python, pandas and      Historical strategy
                          vectorbt initially      evaluation

  Research orchestration  Python initially        Agent tasks and
                                                  synthesis

  Database                PostgreSQL              Durable application and
                                                  research data

  Time-series             PostgreSQL              Market bars and
                          tables/TimescaleDB if   time-series queries
                          needed                  

  Cache/queue             Redis only when         Caching and
                          justified               asynchronous jobs

  Charts                  TradingView Lightweight Interactive market
                          Charts                  charts

  Deployment              Docker Compose          Repeatable local
                          initially               development and
                                                  deployment
  -----------------------------------------------------------------------

Avoid Kafka, Kubernetes and a large microservice fleet in the first
milestone. Begin with a modular monolith plus background workers. Split
services only when there is a measurable reason.

### High-level flow

``` text
Market / SEC / Macro / News Providers
                 |
                 v
          Ingestion Workers
                 |
                 v
      Normalize + Validate + Store
                 |
        +--------+---------+
        |                  |
        v                  v
   Quant Engine       Event/News Index
        |                  |
        +--------+---------+
                 |
                 v
          Research Coordinator
                 |
       +---------+----------+
       |         |          |
       v         v          v
   Technical  Fundamental  News
       |         |          |
       +---------+----------+
                 |
          Bearish Challenger
                 |
             Risk Agent
                 |
                 v
       Deterministic Validation
                 |
                 v
        Evidence-backed Report
                 |
       +---------+----------+
       |                    |
       v                    v
   Web Dashboard       Paper Tracking

Research agents <----> Approved external services
                          |
                          v
                       Axiom Pay
                          |
                     NEAR integration
```

### Suggested repository structure

``` text
axiom/
├── apps/
│   ├── web/                    # Next.js dashboard
│   └── api/                    # Rust Axum API
├── services/
│   ├── ingestion/              # Market, SEC and news ingestion
│   ├── quant-engine/            # Python indicators and screening
│   ├── research-engine/         # Agent orchestration
│   ├── validation-engine/      # Rules and signal validation
│   └── backtest-engine/         # Historical testing
├── packages/
│   ├── shared-types/
│   └── api-client/
├── infrastructure/
│   ├── docker/
│   └── database/
│       └── migrations/
├── config/
│   ├── watchlists/
│   └── strategies/
├── docs/
│   ├── architecture.md
│   ├── data-sources.md
│   ├── signal-methodology.md
│   ├── risk-policy.md
│   └── near-integration.md
├── scripts/
├── .env.example
├── docker-compose.yml
├── HACKATHON.md
└── README.md
```

------------------------------------------------------------------------

## 9. Core data model

Initial relational entities:

  -----------------------------------------------------------------------
  Entity                              Purpose
  ----------------------------------- -----------------------------------
  `assets`                            Tickers, exchanges and company
                                      identifiers

  `watchlists`                        User-defined instrument collections

  `market_bars`                       OHLCV bars with timeframe and
                                      provider

  `market_quotes`                     Quote snapshots and feed metadata

  `financial_statements`              Normalized company financial facts

  `sec_filings`                       Filing metadata, accession number
                                      and source URL

  `news_articles`                     News metadata, timestamps and
                                      source URL

  `economic_events`                   Macro releases and scheduled events

  `indicator_snapshots`               Computed indicators and calculation
                                      version

  `research_runs`                     Each research request and its
                                      configuration

  `agent_runs`                        Agent identity, task, status and
                                      output

  `research_evidence`                 Claims linked to
                                      supporting/contradicting sources

  `trade_signals`                     Published recommendation and
                                      horizon

  `signal_outcomes`                   Subsequent performance and
                                      evaluation

  `strategy_versions`                 Versioned strategy parameters

  `backtest_runs`                     Backtest configuration and metrics

  `paper_positions`                   Simulated positions

  `service_providers`                 Approved external intelligence
                                      providers

  `service_requests`                  Agent requests, quotes and returned
                                      results

  `payments`                          Payment state, network and
                                      transaction reference

  `audit_events`                      Security- and permission-relevant
                                      activity
  -----------------------------------------------------------------------

Store timestamps in UTC internally and render them in the user's
selected timezone. Preserve provider timestamps and retrieval timestamps
separately.

A research run should be reproducible as far as possible: store the
strategy version, prompt/model version, input data references, source
records and validation outcome.

------------------------------------------------------------------------

## 10. Backtesting and performance integrity

Backtesting is a product capability, not a marketing decoration.

### Minimum safeguards

-   Avoid look-ahead bias.
-   Avoid survivorship bias where historical universes are used.
-   Use point-in-time financial data.
-   Include commissions, spread and slippage assumptions.
-   Account for corporate actions and splits.
-   Respect market sessions and time zones.
-   Separate training, validation and out-of-sample periods.
-   Do not tune repeatedly against the final test period.
-   Keep a record of strategy and parameter versions.

### Metrics

-   Win rate.
-   Profit factor.
-   Expectancy per trade.
-   Sharpe ratio.
-   Sortino ratio.
-   Maximum drawdown.
-   Average gain and loss.
-   Average holding period.
-   Number of trades and sample period.

Always show the sample size and test period. Historical performance does
not guarantee future results.

### Paper-trading ledger

Every recommendation should be recorded whether it later wins or loses.
Do not selectively report successful calls. Define entry, exit,
invalidation and evaluation rules before observing the outcome.

------------------------------------------------------------------------

## 11. Security, privacy and operational safeguards

-   Keep API keys and private keys server-side.
-   Use environment variables or a secrets manager; never commit
    secrets.
-   Separate development, testnet and production credentials.
-   Apply least-privilege access to every provider and agent.
-   Treat external documents and web content as untrusted inputs.
-   Restrict agent tools to explicit allowlists.
-   Validate and sanitize provider responses.
-   Add rate limits, timeouts and retry policies.
-   Log agent tool calls and payment actions.
-   Require explicit authorization for payments above policy limits.
-   Do not provide brokerage order permissions in the initial product.
-   Provide a clear audit trail for recommendations and transactions.
-   Make data-provider limitations visible to users.

This product provides informational research. Before public release,
obtain appropriate legal and compliance review for the jurisdictions and
services in which it will operate.

------------------------------------------------------------------------

## 12. Hackathon MVP scope

The hackathon version should demonstrate one end-to-end experience
rather than many incomplete screens.

### Must-have

1.  A working Axiom dashboard.
2.  A curated watchlist of approximately 5--10 stocks for the demo.
3.  Market data and at least one official/company information source.
4.  A research request for a selected ticker.
5.  At least three meaningful specialist-agent roles.
6.  A bearish challenge or explicit counter-evidence step.
7.  A structured research report with linked sources and timestamps.
8.  Deterministic validation of data freshness and risk conditions.
9.  Axiom Pay flow demonstrating a real, verifiable NEAR-powered service
    payment, if supported by the selected integration.
10. Public repository, clear setup instructions and a short demo.

### Should-have

-   Historical chart and a small indicator set.
-   Research history.
-   Basic paper outcome recording.
-   Agent activity display.
-   Payment history and spending-policy display.

### Defer until after the hackathon

-   Full 30--50 stock continuous monitoring.
-   Sophisticated multi-strategy backtesting.
-   Calibrated predictive probabilities.
-   Telegram notifications.
-   User subscriptions and billing.
-   Open provider marketplace.
-   Automated portfolio optimization.
-   Brokerage connectivity or live execution.
-   Large-scale distributed agent infrastructure.

### Demo scenario

A user asks: **"Research NVIDIA for a swing-trading opportunity."**

Axiom should:

1.  Retrieve available price and company information.
2.  Show the data source and freshness.
3.  Run technical, fundamental and news research.
4.  Ask the Bearish Challenger to identify reasons the trade may fail.
5.  Identify a missing piece of information.
6.  Use Axiom Pay to request an approved external service and complete a
    verifiable NEAR payment.
7.  Incorporate the returned information.
8.  Run deterministic validation.
9.  Produce a report with a recommendation or WAIT/AVOID outcome,
    evidence, risks and invalidation conditions.
10. Show the payment record and research provenance.

The demo must remain honest if the data is delayed, incomplete or
unavailable. A WAIT result is acceptable if that is what the evidence
supports.

------------------------------------------------------------------------

## 13. Delivery roadmap

### Phase 0 --- Foundation

-   Create repository and development environment.
-   Define shared schemas and API contracts.
-   Configure PostgreSQL and migrations.
-   Add `.env.example` and secure configuration handling.
-   Document data sources and limitations.

### Phase 1 --- Market data

-   Implement asset/watchlist records.
-   Integrate one market-data provider.
-   Ingest historical bars.
-   Integrate SEC filing metadata.
-   Add source and timestamp tracking.

### Phase 2 --- Research engine

-   Implement technical calculations.
-   Implement fundamental and news retrieval.
-   Define agent input/output schemas.
-   Implement Coordinator, Technical, Fundamental, News and Bearish
    roles.
-   Persist agent runs and evidence.

### Phase 3 --- Validation and reports

-   Define strategy-specific rules.
-   Add data freshness and risk gates.
-   Generate structured reports.
-   Persist recommendations and strategy versions.

### Phase 4 --- NEAR and Axiom Pay

-   Select the supported NEAR integration path.
-   Build a minimal external intelligence service.
-   Add provider allowlisting and spending policy.
-   Complete and verify a payment flow.
-   Record transaction and service-result references.

### Phase 5 --- Dashboard and demo

-   Build overview and research screens.
-   Show agent activity and disagreement.
-   Display evidence, warnings and timestamps.
-   Add payment history.
-   Test the complete demo workflow.
-   Finalize README, HACKATHON.md and public demo post.

### Phase 6 --- Post-hackathon product

-   Expand watchlist and provider coverage.
-   Add continuous monitoring and alerts.
-   Improve backtesting and paper tracking.
-   Add long-term fundamental research.
-   Introduce calibrated performance reporting.
-   Expand provider and agent-service capabilities.
-   Harden security, reliability and deployment.

------------------------------------------------------------------------

## 14. Initial acceptance criteria

The first milestone is complete when:

-   A developer can run the application using documented setup steps.
-   A user can select a supported ticker and request research.
-   The system retrieves and timestamps real source data.
-   Multiple specialist agents return structured findings.
-   The bearish challenge is visible in the report.
-   Every material factual claim has a source or is clearly labelled as
    an interpretation.
-   The validation layer can reject stale or insufficient evidence.
-   A report is stored and can be reopened.
-   A NEAR payment demonstration can be independently verified, if
    included in the submitted MVP.
-   No stock order can be placed by the system.
-   The demo can be repeated reliably from a clean setup.

------------------------------------------------------------------------

## 15. Product principles

1.  **Evidence before eloquence:** a polished explanation is not a
    substitute for reliable data.
2.  **Disagreement is useful:** agents should challenge assumptions, not
    simply reinforce one another.
3.  **Time horizon matters:** every recommendation must state the
    strategy and horizon.
4.  **Deterministic calculations:** financial metrics and risk checks
    belong in tested code.
5.  **Transparent uncertainty:** missing, delayed or conflicting
    evidence must be visible.
6.  **No fabricated precision:** do not invent probabilities, prices,
    targets or source claims.
7.  **Accountability:** preserve every recommendation and evaluate
    losses as well as wins.
8.  **Controlled autonomy:** agents may research and purchase approved
    services within limits; they may not trade stocks.
9.  **Composable architecture:** build the MVP so future agents,
    providers and strategies can be added without rewriting the
    platform.
10. **Build the real product:** hackathon features should become durable
    product capabilities, not throwaway demo tricks.

------------------------------------------------------------------------

## 16. Immediate build checklist

-   [ ] Create the `axiom` repository.
-   [ ] Add this product specification to `docs/product-spec.md`.
-   [ ] Create the monorepo structure.
-   [ ] Define shared types for assets, bars, agent findings, evidence
    and reports.
-   [ ] Configure PostgreSQL and the first migrations.
-   [ ] Select and verify the initial market-data provider.
-   [ ] Build one ticker research flow end-to-end.
-   [ ] Implement the first three agent roles.
-   [ ] Add source provenance and freshness checks.
-   [ ] Build the report UI.
-   [ ] Validate the NEAR payment path against current official
    documentation.
-   [ ] Add a real external-service payment demonstration.
-   [ ] Record a repeatable demo and complete hackathon documentation.

**Definition of success:** Axiom can take a stock research request,
gather real evidence, coordinate agents that can disagree, validate the
result, and return a transparent report. Its agent economy layer
demonstrates a controlled, verifiable way to obtain external
intelligence through NEAR.
