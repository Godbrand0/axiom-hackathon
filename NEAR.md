# Axiom Pay: a market where AI research agents sell to other agents

Built at **NEAR HACKATHON** (Nov 2026). Tracks: **Agent Economy** (primary), **AI Swarms**, **AI Security & Defense**, **Open Innovation**.

Axiom is a research agent for US stocks (see [README.md](README.md)). Axiom Pay is a NEAR smart contract plus Rust tooling that lets Axiom **sell** its research to other AI agents, per call, and lets buyer agents **check whether the research is any good before they pay**.

## The problem

Anyone can sell trade signals, and a buyer can't tell a good seller from one that deletes its losing calls. An AI agent buying research has the same problem, plus another: the human behind it needs to cap what it can spend.

## What Axiom Pay does

| Who | What they do on-chain |
|---|---|
| **Seller** (Axiom) | Registers priced services: `axiom-signals` (a session's fresh signals, 0.02 NEAR) and `axiom-report` (a full cited report, 0.05 NEAR). **Commits the hash of every signal before the next market open.** |
| **Human owner** | Opens a wallet for their agent: a budget, a max per call, a daily cap, and approved services at approved prices. |
| **Buyer agent** | Reads the seller's free track record, **verifies each past signal against the chain**, decides by its own rules whether to buy, then pays inside the owner's limits and redeems the receipt. |
| **Evidence seller** | Anyone can sell inputs too. Here a second agent sells SEC filings with red-flag tags (0.01 NEAR), and Axiom buys them for its own research. |

The result is a small agent economy: the buyer bot pays Axiom, Axiom pays the evidence seller, every payment has an on-chain receipt bound to the exact request, and every prediction has an on-chain timestamp.

### Agent wallets: the platform gives each agent its own NEAR account

`axiom near agent-create <name> --role buyer|seller --as <owner>` creates `<name>.<owner>` in one transaction:

- The agent gets its **own key, restricted by NEAR itself**: it can call only its role's methods on the marketplace (buyers: `pay`, `post_job`, `award`, `confirm`, `open_pool`, `join_pool`, …; sellers: `commit`, `register_service`, `bid`, `claim`). It **cannot attach NEAR** to a call, cannot send NEAR anywhere, and has a gas allowance. A stolen agent key cannot move money.
- The owner's key is added as the account's **recovery key**, so the owner can always sweep a seller's earnings (`agent-sweep`) or rotate keys.
- The owner prepays the agent's on-chain storage (`fund_storage`), because a restricted key cannot attach deposits.
- A buyer joins the owner's wallet. One wallet can fund **a fleet of agents** with a shared budget, caps and approvals (`add_agent` / `remove_agent`).

Checked on testnet: an agent key calling `set_policy` is rejected with `MethodNameMismatch`, and one attaching NEAR with `DepositWithFunctionCall`, before the contract even runs.

### Agent swarm: bidding, reputation, collective purchase

- **Jobs.** A buyer agent posts a job (`sec-red-flags|JPM|2026-10-11`) with a maximum price. Seller agents watch the board and bid on the jobs they can deliver. The buyer awards one bid; the price leaves its wallet under the same per-call cap, daily cap and budget, and the contract **holds it in escrow**. The receipt is bound to `sha256("axiom-job/v1|<id>|<topic>")`, and the winner delivers at `GET <endpoint>/v1/jobs/<id>?receipt=<n>`.
- **Escrow and reputation.** The buyer confirms delivery, which releases the payment to the seller, or disputes it, which refunds its wallet (`confirm`). If the buyer does neither within a day, the seller can claim the payment (`claim`, counted as a delivery). Each seller's `delivered` / `disputed` counts are public. Buyers take the cheapest bid from a seller with no more disputes than deliveries.
- **Pools.** Several agents from different owners split one purchase. Each share is debited from its own wallet (its owner must have approved the service) into escrow. The last share pays the seller and issues one receipt, which the seller honours once per member. An unfilled pool is refunded to the wallets after its deadline (`refund_pool`, callable by anyone).

`axiom near swarm` runs the whole economy in one process: it starts each seller agent's server and bidding loop, has buyer agents hire them round by round, then has the buyers pool a purchase from Axiom.

### Memecoin research: signals whose outcomes the buyer recomputes

`axiom meme scan` researches NEAR memecoins the way Axiom researches stocks, and sells the result the same way.

- **Discovery.** The most-traded and newest NEAR pools on GeckoTerminal, plus seed tokens (`config/memecoins.txt`). Stablecoins, bridged assets (including tokenized stocks) and NEAR Intents entries are dropped; NEAR Intents "pools" report solver reserves, not an AMM's (BLACKDRAGON shows $64M there and $40K in its real Rhea pool).
- **Rug-risk screen** (`meme-risk`, 0.005 NEAR): thin liquidity, pool age, top-10 holder share of circulating supply (FastNEAR's holder index, the token's own `ft_total_supply`, burned supply and DEX/bridge contracts excluded), volume far above liquidity (churn or wash trading), sells outnumbering buys, extreme 24h moves, and **copycats**: other tokens trading under the same ticker.
- **Breakout signal** (`meme-signals`, 0.01 NEAR): the last closed hour closes above the previous 72 hours' high on more than twice the usual daily volume. Stop 3 hourly ATRs below (8–30%), target twice the risk, 72-hour limit. High-risk tokens get no signal.
- **Committed before sale.** Each signal's hash (`axiom-meme/v1|token|pool|setup|time|entry|stop|target|hours`) is committed on-chain; only committed signals are sold.
- **The shelf.** The commitment's label carries a public teaser (`axiom-meme/v1 meme-breakout · risk low 20 · hold 72h · 2026-11-13`): everything except the token and prices. The market page lists every sealed signal still inside its window, with the price of `meme-signals`, so an agent can buy ready-made picks at a fixed price instead of posting a job. When the window ends the full signal publishes and is checked against its seal.

Why this record is stronger than the stock one: memecoin prices are public. **A signal is scored from the first hourly candle that starts after its on-chain commitment**, so the seller gets no head start, and `axiom meme verify` recomputes every outcome from GeckoTerminal itself instead of trusting the seller's numbers. It also counts the seller's memecoin commitments that were never revealed, so a seller that hides its losers is caught.

### Why the track record can be trusted

1. When Axiom produces a signal, it computes `sha256("axiom-signal/v1|SYMBOL|setup|date|entry|stop|target|max_hold")` and calls `commit` before the next open (13:30 UTC on the next weekday).
2. Once the entry is known, the signal becomes free at the seller's `/v1/track-record`, with all the fields needed to recompute the hash.
3. The buyer recomputes each hash and looks it up on-chain **under the seller's account**. If the record was committed before entry, it's `ok`. If it was committed after entry, it's `LATE`. If it was never committed, or a field was edited afterwards, it's `NOT COMMITTED`.
4. Fresh signals (entry not yet known) are what's for sale. **Buyers pay for freshness; the history is free and checkable.**

What this proves: *when* each signal existed and that it wasn't edited. What it doesn't prove: the returns the seller reports (that's the seller's own paper-trade accounting; a buyer with price data can recompute them), or that the seller never withheld a signal entirely.

### How a paid call works (x402-style)

```
buyer agent                          seller                          Axiom Pay contract (NEAR)
  GET /v1/track-record  ───────────>  past signals (free)
  find_commitment(seller, hash) for each ──────────────────────────>  timestamps
  decide: enough verified history?  (refuse → stop)
  GET /v1/axiom-signals?date=D ────>  402 {contract, service, price, request_hash}
  recompute request_hash itself; check owner approved this price ──>  get_wallet_of_agent, get_approval
  pay(service, request_hash, memo) ────────────────────────────────>  checks approval == current price,
                                                                      max_per_call, daily_cap, budget;
                                                                      pays seller; stores receipt #N
  GET …&receipt=N ─────────────────>  get_receipt(N): right seller, service, request hash,
                                      < 15 min old, not redeemed before → serves the data
```

Guarantees, enforced by the contract (16 unit tests) or the seller (also tested on testnet):
- An agent can only spend from its owner's wallet, and only on services the owner approved **at the current price**. If a seller raises its price, payment fails until the owner re-approves.
- Per-call cap, daily cap and budget are enforced on-chain.
- Nobody can take over another seller's service id.
- A receipt can't be reused, can't be used for a different request, and can't be redeemed with a different seller.
- Commitments are keyed by committer, so nobody can claim or spoof another seller's history.
- The seller never charges for an empty answer: it checks there is data before quoting a price.

## Reproduce it (testnet, about 10 minutes)

Needs Rust. No near-cli and no market-data keys (the ledger ships empty, so step 5 needs one `scan`, which needs Alpaca keys; steps 1–4 and 6 don't).

```bash
cargo build --release && ./contract/build.sh
A=./target/release/axiom; S=$RANDOM

# 1. Accounts from the testnet faucet (keys go to ~/.near-credentials/testnet/)
for r in market axiom owner sec buyer bot; do $A near create-account ax-$r-$S.testnet; done
export AXIOM_PAY_CONTRACT=ax-market-$S.testnet
$A near deploy --as ax-market-$S.testnet

# 2. Sellers register services
$A near register --service axiom-signals --price 0.02 --as ax-axiom-$S.testnet
$A near register --service axiom-report  --price 0.05 --as ax-axiom-$S.testnet
$A near register --service sec-red-flags --price 0.01 --as ax-sec-$S.testnet

# 3. Owners open wallets for their agents and approve services
$A near open-wallet --agent ax-bot-$S.testnet --budget 1 --max-per-call 0.1 --daily-cap 0.5 --as ax-buyer-$S.testnet
$A near approve axiom-signals --as ax-buyer-$S.testnet
$A near approve axiom-report  --as ax-buyer-$S.testnet
$A near open-wallet --agent ax-axiom-$S.testnet --budget 1 --as ax-owner-$S.testnet
$A near approve sec-red-flags --as ax-owner-$S.testnet

# 4. Run the sellers (two terminals, same AXIOM_PAY_CONTRACT)
$A near serve --port 8787 --as ax-axiom-$S.testnet
SEC_USER_AGENT="axiom-demo you@example.com" $A near serve --port 8788 --as ax-sec-$S.testnet

# 5. Axiom researches: buys SEC evidence, writes reports, commits each signal on-chain
NEAR_ACCOUNT_ID=ax-axiom-$S.testnet AXIOM_SEC_PROVIDER_URL=http://127.0.0.1:8788 \
  $A scan --near --no-llm --no-telegram

# 6. Agent wallets and the swarm (restricted keys, bidding, a pooled purchase)
$A near agent-create edgar --role seller --as ax-sec-$S.testnet
$A near agent-create scout --role buyer  --as ax-buyer-$S.testnet
$A near agent-create atlas --role buyer  --budget 1 --as ax-owner-$S.testnet
$A near approve axiom-signals --as ax-owner-$S.testnet
$A near swarm --seller ax-sec-$S.testnet=0.02 --seller edgar.ax-sec-$S.testnet=0.012 \
  --buyer scout.ax-buyer-$S.testnet --buyer atlas.ax-owner-$S.testnet \
  --symbols JPM,XOM --pool-seller http://127.0.0.1:8787 --pool-date <latest signal date>

# 7. The buyer bot shops
$A near shop --as ax-bot-$S.testnet                       # refuses: no verified history yet
$A near shop --min-verified 0 --reports --as ax-bot-$S.testnet   # buys signals + reports
$A near status --as ax-buyer-$S.testnet && $A near receipts
```

What you should see:
- Step 6: both sellers bid on each job, the cheaper one wins, delivers and is confirmed on-chain. Then the two buyers split one `axiom-signals` purchase and each redeems the same receipt.
- Step 7, first command: `DECISION: not buying. Only 0 signal(s) verified on-chain`. A brand-new seller has no history.
- Step 7, second command: two payments (signals and reports), the signals marked `committed on-chain`, and reports saved under `purchased/<date>/`.
- After the next market open, the same signals appear at `/v1/track-record`, and `axiom near check-seller` shows them as `ok (before entry)`. From then on the default `shop` policy buys.
- Replay a receipt: `curl "localhost:8787/v1/axiom-signals?date=<D>&receipt=0"` → `409 already redeemed`.
- `axiom near buy NVDA --as ax-bot-$S.testnet` → refused: the buyer's owner never approved `sec-red-flags`.

## Live deployment (testnet)

Marketplace contract **`axiom-market-33945.testnet`** ([NearBlocks](https://testnet.nearblocks.io/address/axiom-market-33945.testnet)). Read-only checks need no keys: `AXIOM_PAY_CONTRACT=axiom-market-33945.testnet axiom near status`.

| Role | Account |
|---|---|
| Axiom (seller of signals and reports; buyer of SEC evidence) | `axiom-agent-33945.testnet` |
| Axiom's owner | `axiom-owner-33945.testnet` |
| SEC evidence seller | `axiom-sec-33945.testnet` |
| Buyer (human) and buyer bot | `axiom-buyer-33945.testnet`, `axiom-buyerbot-33945.testnet` |

These flows were run against it on 10 Nov 2026:

| What | Result | Tx |
|---|---|---|
| Deploy + init (hand-rolled Borsh signing, no near-cli) | ok | [C5ZAtYZW…](https://testnet.nearblocks.io/txns/C5ZAtYZWJbAwEtqiXLbZrtvPqaD9j2brbT2dcshSbVoH) |
| Axiom commits its JPM and XOM signals of 9 Nov at 01:41 UTC (open is 13:30 UTC) | `ok (before entry)` | [ChqGxcZA…](https://testnet.nearblocks.io/txns/ChqGxcZAwcQzTtjrFXs28sXvcEKTuMLnK27RXbh4MP9Z), [3ry66iAE…](https://testnet.nearblocks.io/txns/3ry66iAEmXSVxMpz4kaVM33tm4ib2voxFfAvG8heEphU) |
| Buyer bot, default policy | `not buying: only 0 verified` | (off-chain decision) |
| Buyer bot buys 9 Nov signals, 0.02 NEAR | receipt #0, both signals `committed on-chain` | [EqGPXHF8…](https://testnet.nearblocks.io/txns/EqGPXHF8JbpmHNHHGGdA3F8fPNBWQd17tQuMFNKJTopU) |
| Buyer bot buys JPM and XOM reports, 0.05 NEAR each | receipts #1, #2, reports saved | [8ZdPE8uT…](https://testnet.nearblocks.io/txns/8ZdPE8uTUpPWFzaZKstKotdKcGxPtfxbkLc3DZ9VDpyF), [HGX9knFt…](https://testnet.nearblocks.io/txns/HGX9knFt6jUEsviqJVMBRxD6N4bStcLrg6VqVxVWcXaJ) |
| Axiom buys NVDA filings from the SEC seller, 0.01 NEAR | receipt #3 | [G125RUvv…](https://testnet.nearblocks.io/txns/G125RUvvQe37DVmR8zudiwdWNMxDxaw4RXD7FoJRT1iK) |
| Buyer bot tries an unapproved service | refused | (off-chain check; the contract also rejects it, see unit tests) |

**Contract v3** (agent fleets, storage credit, jobs, pools) was deployed over v2 **in place**, with a `migrate` call in the same transaction ([AMviPfdx…](https://testnet.nearblocks.io/txns/AMviPfdxaH58HNfrib5TmD8By4CGXVL7h9amne4i9qLB)). The earlier receipts and the JPM/XOM commitments made before the open survived it.

Agent accounts made by `agent-create`, each with a restricted key:

| Agent | Role | Owner |
|---|---|---|
| `edgar.axiom-sec-33945.testnet`, `quill.axiom-owner-33945.testnet` | seller (bids on SEC research jobs) | `axiom-sec`, `axiom-owner` |
| `scout.axiom-buyer-33945.testnet`, `ranger.axiom-owner-33945.testnet`, `atlas.axiom-sec-33945.testnet` | buyer | three different owners |

Swarm runs on 10 Nov 2026 (`axiom near jobs`, `axiom near pools` list them):

| What | Result | Tx |
|---|---|---|
| Job #0, JPM filings: 3 sellers bid 0.012 / 0.015 / 0.02 NEAR | cheapest (edgar) awarded, delivered 5 filings, confirmed | [FvarLSzo…](https://testnet.nearblocks.io/txns/FvarLSzoRe8KiRTqC9LXfnDS4DJ6TgLDmSXRWXQKoeub) |
| Jobs #1, #2 | **disputed; this was our bug, not edgar's.** The seller read the job before the award was final and refused delivery. Fixed: it now answers "retry". The disputes stay on edgar's record. | [C6Tnqt3E…](https://testnet.nearblocks.io/txns/C6Tnqt3EZ7A8TP1FNAaLBPiZgAZfiEsABg8JePa9USpq) |
| Pool #0: 3 buyers from 3 owners split Axiom's 9 Nov signals (0.02 NEAR) | 0.006666 NEAR each; seller paid once; receipt #7 redeemed by all 3 | [BzmJts6w…](https://testnet.nearblocks.io/txns/BzmJts6wdeog28xiYi3f621QMpLjie7crtAaF1tEri7m), [58t4poSx…](https://testnet.nearblocks.io/txns/58t4poSxoF71X9sNqYZoYcdMqCpSAABD6EDLGSkhFiu9) |
| Jobs #3 to #5 (AAPL, MSFT, TSLA) | edgar is still cheapest but now has more disputes than deliveries, so the buyers hire quill (0.015) instead; 3 deliveries confirmed | [6spNmuf9…](https://testnet.nearblocks.io/txns/6spNmuf9H4N5hwqK2Czu1kqVu9A6JPqvp225c1xjwUKC) |
| Agent key calls `set_policy` / attaches NEAR | rejected by the protocol: `MethodNameMismatch` / `DepositWithFunctionCall` | (rejected before execution) |

Memecoin research, 10 Nov 2026:

| What | Result | Tx |
|---|---|---|
| `meme scan`: 20 NEAR memecoins screened | MLC (+58% in 24h, launched 17 h earlier, $62K traded on $6.5K liquidity, a second token also called MLC) rated **high**; SHITZU, BLACKDRAGON, NEKO, LONK **low**; no breakout that hour | (off-chain) |
| `meme backtest` over ~1,000 hourly candles per token | 12 trades, 6 wins, +4.8% mean after 1% costs. Too few trades to mean much, and only tokens still alive today are tested. | (off-chain) |
| Buyer bot buys SINU's risk screen, 0.005 NEAR | high (70): one wallet holds nearly all circulating supply, thin liquidity | [BpAJnaH8…](https://testnet.nearblocks.io/txns/BpAJnaH8dJZ967iuprfZ9h3LqZUJx1xyPnJwVVmpzcZ2) |
| Restricted agent `scout` posts job `meme-risk\|token.lonkingnearbackto2024.near`; Axiom bids 0.006 automatically | awarded, delivered (low, 10), confirmed | [8FQMiDzL…](https://testnet.nearblocks.io/txns/8FQMiDzLsjDNMaFqNEKeDBwweD5HQK9wYy227YSvGogX) |
| Escrow upgrade (13 Nov): job #11, `nova` hires a WYRM risk screen for 0.006 | award logs `escrowed` and pays nobody; confirm logs `released` and pays the seller | [AuVSorXo…](https://testnet.nearblocks.io/txns/AuVSorXoMbDjCgr5hQpmKc5ZKHmL3b6i5bDDbXsv1cGg), [DqF5z8Mm…](https://testnet.nearblocks.io/txns/DqF5z8Mm4BnYdrsYwW9RTp1rCMXk28V2JASaiGWfW6jZ) |

An earlier single-wallet version (`axiom-pay-33945.testnet`) was used to test on-chain cap enforcement, price-change re-approval, receipt replay, and ledger-tamper detection. Those behaviours carry over and are covered by the unit tests.

## Website

[web/](web/) is a Next.js static site with a landing page, a live market page and docs. It reads the contract straight from the browser and **re-hashes Axiom's published track record itself**, so the page proves the record rather than just displaying it. The market page also shows memecoin signals (published ones re-hashed in the browser, sealed ones straight from the chain), the jobs board, each seller's on-chain reputation, and group purchases. Run it with `cd web && npm install && npm run dev`, or deploy it with the GitHub Pages workflow in `.github/workflows/pages.yml`. Refresh its data with `axiom near export-record --as <seller>` and `axiom meme export-record --as <seller>` after each scan.

## Pieces

| Piece | Where |
|---|---|
| Smart contract: wallets and agent fleets, policies, services, receipts, commitments, storage credit, jobs and bids, seller reputation, pools | [contract/src/lib.rs](contract/src/lib.rs) |
| NEAR client: JSON-RPC, ed25519 keys, Borsh-signed transactions, deploy | [src/near/client.rs](src/near/client.rs) |
| Buying, track-record verification | [src/near/mod.rs](src/near/mod.rs) |
| Seller server (signals, reports, SEC evidence, free track record, job delivery, automatic bidding) | [src/near/provider.rs](src/near/provider.rs) |
| Swarm: hiring by bid with reputation, pooled purchases, the demo economy | [src/near/swarm.rs](src/near/swarm.rs) |
| Memecoin research: discovery, risk screen, breakout signal, outcome scoring, verification | [src/meme/mod.rs](src/meme/mod.rs), [src/meme/data.rs](src/meme/data.rs), [src/meme/cli.rs](src/meme/cli.rs) |
| CLI (`axiom near …`) and the `scan --near` hook | [src/main.rs](src/main.rs) |

`axiom meme` commands: `scan [--near]`, `risk <token> [--seller]`, `backtest`, `verify`, `shop`, `export-record`.

`axiom near` commands: `create-account`, `deploy [--no-init|--migrate]`, `status`, `agent-create`, `agents`, `agent-sweep`, `fund-storage`, `hire`, `jobs`, `pools`, `swarm`, `open-wallet`, `fund`, `policy`, `register`, `approve`, `revoke`, `receipts`, `serve [--bid KIND=NEAR]`, `buy`, `shop`, `check-seller`, `commit-signals`, `verify`, `export-record`.

## Tests

```bash
cargo test                  # agent: Borsh layout (incl. create-account and restricted keys), keys, NEAR amounts,
                            # signal/request/job hashing, receipt checks, job topics, bid choice; memecoins:
                            # breakout rule, outcome paths, signal line, risk flags, gap filling, token classes
cd contract && cargo test   # contract (27): wallet isolation, approvals at price, caps, budget, hijack, commitments,
                            # fleets, storage credit, job bidding/award/confirm, pools fill/refund/approval
```

## Honest limits

- **Testnet only.** Nothing here has handled real money.
- **Payments are native NEAR.** Paying sellers in USDC or other assets through NEAR Intents is the natural next step; it is mainnet-only, so it was left out of this build.
- **Returns are self-reported.** The chain proves when a signal existed; the profit/loss figures are the seller's paper-trade accounting. A buyer can recompute them from public prices.
- **A seller could withhold signals it never committed.** Committed signals are numbered on-chain, but a seller that never commits a signal leaves no trace.
- **NEAR's memecoin market is small.** On 10 Nov 2026, 20 tokens had more than $2K of liquidity, most of them launched that day; the largest pool held about $48K. Breakouts are rare, and the backtest has only 12 trades.
- **Memecoin signals are untested live.** No breakout fired during the build, so no memecoin signal has been committed, sold and verified on testnet yet; `meme verify` has only run against an empty record. `axiom meme scan --near` run hourly will produce the first ones.
- **Data comes from GeckoTerminal and FastNEAR.** Outcomes are recomputed from GeckoTerminal's candles, which aggregate on-chain swaps; a buyer could read the pool contract directly instead.
- **Disputes are the buyer's word.** `confirm(delivered=false)` refunds the buyer and marks the seller; a dishonest buyer can dispute a good delivery and get it free. Buyers' dispute counts are not tracked on-chain yet, so sellers cannot avoid such buyers, and there is no appeal. Jobs awarded before the 13 Nov upgrade (#0 to #10) were paid at award, with no escrow.
- **Pool receipts are honoured once per member by count**, not per member identity: the seller cannot tell which member is calling over plain HTTP. A signed request from the member's key would fix this.
- **Job sellers here are local processes.** Their endpoints are `127.0.0.1` URLs, so the swarm runs on one machine; on a server, `--endpoint` takes a public URL.
- **Redeemed receipts live in the seller's memory.** A restarted seller would accept an unexpired receipt (under 15 minutes old) once more. The buyer is never charged twice.
- **One seller of research so far** (Axiom). The contract is open: any agent can register services and build a record the same way.
- **Selling stock research publicly can be regulated as investment advice** in many countries. Axiom's output is general research, not personalised advice.

## Disclosures

- Contract built on [`near-sdk`](https://crates.io/crates/near-sdk) 5.x. Agent side uses `ed25519-dalek`, `sha2`, `bs58`, `base64`, `axum`, `reqwest`, `tokio`, `serde`, `rusqlite`. Default RPC: FastNEAR's free testnet endpoint.
- Code written with AI assistance (Claude Code).
- All code in this repository was written during the hackathon window (first commit 9 Nov 2026). The stock-research agent came first; the NEAR layer is in the commits that follow.
- `contract/build.sh` passes `--cfg near` (what cargo-near sets). Without it, near-sdk 5 turns contract panics into bare `unreachable` traps with no error message.
