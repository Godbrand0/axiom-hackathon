# Axiom Pay: a market where AI research agents sell to other agents

Built at **NEAR HACKATHON** (Nov 2026). Tracks: **Agent Economy** (primary), **AI Swarms**, **Open Innovation**.

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

# 6. The buyer bot shops
$A near shop --as ax-bot-$S.testnet                       # refuses: no verified history yet
$A near shop --min-verified 0 --reports --as ax-bot-$S.testnet   # buys signals + reports
$A near status --as ax-buyer-$S.testnet && $A near receipts
```

What you should see:
- Step 6, first command: `DECISION: not buying. Only 0 signal(s) verified on-chain`. A brand-new seller has no history.
- Step 6, second command: two payments (signals and reports), the signals marked `committed on-chain`, and reports saved under `purchased/<date>/`.
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

An earlier single-wallet version (`axiom-pay-33945.testnet`) was used to test on-chain cap enforcement, price-change re-approval, receipt replay, and ledger-tamper detection. Those behaviours carry over and are covered by the unit tests.

## Website

[web/](web/) is a Next.js static site with a landing page, a live market page and docs. It reads the contract straight from the browser and **re-hashes Axiom's published track record itself**, so the page proves the record rather than just displaying it. Run it with `cd web && npm install && npm run dev`, or deploy it with the GitHub Pages workflow in `.github/workflows/pages.yml`. Refresh its data with `axiom near export-record --as <seller>` after each scan.

## Pieces

| Piece | Where |
|---|---|
| Smart contract: wallets, policies, services, receipts, commitments | [contract/src/lib.rs](contract/src/lib.rs) |
| NEAR client: JSON-RPC, ed25519 keys, Borsh-signed transactions, deploy | [src/near/client.rs](src/near/client.rs) |
| Buying, track-record verification | [src/near/mod.rs](src/near/mod.rs) |
| Seller server (signals, reports, SEC evidence, free track record) | [src/near/provider.rs](src/near/provider.rs) |
| CLI (`axiom near …`) and the `scan --near` hook | [src/main.rs](src/main.rs) |

`axiom near` commands: `create-account`, `deploy [--no-init]`, `status`, `open-wallet`, `fund`, `policy`, `register`, `approve`, `revoke`, `receipts`, `serve`, `buy`, `shop`, `check-seller`, `commit-signals`, `verify`, `export-record`.

## Tests

```bash
cargo test                  # agent: Borsh layout, keys, NEAR amounts, signal and request hashing, receipt checks
cd contract && cargo test   # contract (16): wallet isolation, approvals at price, caps, budget, hijack, commitments
```

## Honest limits

- **Testnet only.** Nothing here has handled real money.
- **Payments are native NEAR.** Paying sellers in USDC or other assets through NEAR Intents is the natural next step; it is mainnet-only, so it was left out of this build.
- **Returns are self-reported.** The chain proves when a signal existed; the profit/loss figures are the seller's paper-trade accounting. A buyer can recompute them from public prices.
- **A seller could withhold signals it never committed.** Committed signals are numbered on-chain, but a seller that never commits a signal leaves no trace.
- **Redeemed receipts live in the seller's memory.** A restarted seller would accept an unexpired receipt (under 15 minutes old) once more. The buyer is never charged twice.
- **One seller of research so far** (Axiom). The contract is open: any agent can register services and build a record the same way.
- **Selling stock research publicly can be regulated as investment advice** in many countries. Axiom's output is general research, not personalised advice.

## Disclosures

- Contract built on [`near-sdk`](https://crates.io/crates/near-sdk) 5.x. Agent side uses `ed25519-dalek`, `sha2`, `bs58`, `base64`, `axum`, `reqwest`, `tokio`, `serde`, `rusqlite`. Default RPC: FastNEAR's free testnet endpoint.
- Code written with AI assistance (Claude Code).
- All code in this repository was written during the hackathon window (first commit 9 Nov 2026). The stock-research agent came first; the NEAR layer is in the commits that follow.
- `contract/build.sh` passes `--cfg near` (what cargo-near sets). Without it, near-sdk 5 turns contract panics into bare `unreachable` traps with no error message.
