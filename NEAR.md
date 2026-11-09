# Axiom Pay: the NEAR layer of Axiom

Built at **NEAR HACKATHON** (Nov 2026). Tracks: **Agent Economy** (primary), **AI Swarms**, **Open Innovation**.

Axiom is a research agent for US stocks (see [README.md](README.md)). Axiom Pay adds two things an autonomous research agent needs before anyone should trust it with money or with a track record:

1. **A spending policy it cannot break.** A human owner funds a budget on NEAR and sets the limits: which services may be paid, the most per call, and the most per day. The agent buys evidence from other agents per call, inside those limits. Each payment leaves an on-chain receipt bound to the hash of the exact request.
2. **A track record it cannot rewrite.** Every signal Axiom produces is hashed and committed on-chain **before the next market open**, so before its outcome can be known. Anyone can recompute the hash from the ledger and check when it was committed. A signal committed after entry is reported as `LATE`; a signal that was never committed is reported as `NOT COMMITTED`. Backtests can be overfit; a ledger of commitments made before the outcome cannot.

## Pieces

| Piece | Where | What it does |
|---|---|---|
| Smart contract | [contract/src/lib.rs](contract/src/lib.rs) | Owner/agent roles, budget, per-call and daily caps, service registry with owner allowlist, payment receipts, signal commitments. 11 unit tests. |
| NEAR client | [src/near/client.rs](src/near/client.rs) | JSON-RPC views, ed25519 keys, Borsh-encoded signed transactions, deploy. Self-contained: no near-cli needed. |
| Buyer + commitments | [src/near/mod.rs](src/near/mod.rs) | x402-style purchase (ask → `402` terms → check → pay → redeem), signal hashing, `verify`. |
| Provider agent | [src/near/provider.rs](src/near/provider.rs) | HTTP service selling SEC filings with 8-K red-flag codes. Serves a request only against a valid, unexpired, unredeemed receipt for that exact request. |
| Pipeline hook | [src/main.rs](src/main.rs) `scan --near` | Buys filing evidence through Axiom Pay (when `AXIOM_SEC_PROVIDER_URL` is set) and commits each new signal. |

## How a paid call works

```
Axiom agent                         Provider agent                 Axiom Pay contract (NEAR)
    | GET /v1/sec-red-flags?symbol=NVDA&since=…  |                          |
    |------------------------------------------->|                          |
    |   402 {contract, service_id, price, request_hash}                     |
    |<-------------------------------------------|                          |
    | recompute request_hash itself; check service is approved, price      |
    |---------------------------------------------------- view get_service ->|
    |---------------------------------- pay(service_id, request_hash, memo) ->|
    |        contract checks: caller is agent, service approved,            |
    |        price <= max_per_call, spent_today + price <= daily_cap,       |
    |        price <= budget → transfers price to provider → receipt #N     |
    | GET …&receipt=N                            |                          |
    |------------------------------------------->| view get_receipt(N) ---->|
    |                                            | checks provider, service,|
    |                                            | request hash, age<15min, |
    |                                            | not redeemed before      |
    |               filings JSON                 |                          |
    |<-------------------------------------------|                          |
```

What each side cannot do:
- The agent cannot pay an unapproved service, exceed either cap, or spend more than the budget. The contract rejects it.
- A provider cannot raise its price on an approved service: any price change clears the approval until the owner approves again.
- A provider cannot take over another provider's service id.
- A receipt cannot be reused for a different request, a different provider, or a second time.

## Reproduce it (testnet, about 10 minutes)

Needs Rust (`rustup`). No near-cli, no market-data keys. The SEC provider needs only `SEC_USER_AGENT`.

```bash
cargo build --release
A=./target/release/axiom
S=$RANDOM   # unique suffix for account names

# 1. Four funded testnet accounts from the faucet (keys go to ~/.near-credentials/testnet/)
for r in owner agent sec pay; do $A near create-account axiom-$r-$S.testnet; done

# 2. Build and deploy the contract (deploy + init in one transaction)
./contract/build.sh
$A near deploy --as axiom-pay-$S.testnet --owner axiom-owner-$S.testnet --agent axiom-agent-$S.testnet \
  --max-per-call 0.05 --daily-cap 0.2
export AXIOM_PAY_CONTRACT=axiom-pay-$S.testnet

# 3. Owner funds the agent; provider registers; owner approves
$A near fund 1 --as axiom-owner-$S.testnet
$A near register --price 0.01 --as axiom-sec-$S.testnet
$A near approve sec-red-flags --as axiom-owner-$S.testnet
$A near status          # see: budget 1 NEAR, service approved

# 4. Provider agent sells evidence (separate terminal, same AXIOM_PAY_CONTRACT)
SEC_USER_AGENT="axiom-demo you@example.com" $A near serve --as axiom-sec-$S.testnet

# 5. Agent buys NVDA's last 30 days of filings: pays on-chain, redeems the receipt
$A near buy NVDA --as axiom-agent-$S.testnet
$A near receipts        # receipt #0: 0.01 NEAR to the provider, bound to the request hash

# 6. Policy enforcement: the daily cap is 0.2 NEAR, so the 21st 0.01-NEAR call is rejected by the contract
for i in $(seq 1 21); do $A near buy AAPL --days $i --as axiom-agent-$S.testnet || break; done
#   → "Smart contract panicked: daily cap reached"
```

You should see:
- After step 5: the provider's terminal logs `served NVDA … for receipt #0`, and `status` shows `calls 1 earned 0.01 NEAR`. The provider's balance went up by 0.01 NEAR.
- Replaying the same receipt: `curl "localhost:8787/v1/sec-red-flags?symbol=NVDA&since=<date>&receipt=0"` returns `409 receipt #0 already redeemed`.
- Changing the request (`symbol=AAPL&…&receipt=0`) returns `402 … paid for a different request`.

### Verifiable track record (needs the Alpaca keys from the README)

```bash
export NEAR_ACCOUNT_ID=axiom-agent-$S.testnet
export AXIOM_SEC_PROVIDER_URL=http://127.0.0.1:8787   # optional: buy filings instead of fetching them
$A scan --near --no-llm --no-telegram   # each new signal: report + paid evidence + on-chain commitment
$A near verify                          # every ledger signal vs its commitment and timestamp
```

`verify` recomputes `sha256("axiom-signal/v1|SYMBOL|setup|as_of|entry|stop|target|max_hold")` from the local ledger, looks it up on-chain, and checks that the commitment's block time is before 13:30 UTC on the fill day (the earliest the next open can be known). Editing any field in the ledger after the fact makes the hash disappear from the chain. Each report also ends with its commitment line and explorer link.

## Live deployment (testnet)

Contract **`axiom-pay-33945.testnet`**. Anyone can check it with `AXIOM_PAY_CONTRACT=axiom-pay-33945.testnet axiom near status` (no keys needed) or on [NearBlocks](https://testnet.nearblocks.io/address/axiom-pay-33945.testnet).

| Role | Account |
|---|---|
| Owner | `axiom-owner-33945.testnet` |
| Agent (Axiom) | `axiom-agent-33945.testnet` |
| Provider (SEC red flags) | `axiom-sec-33945.testnet` |

These flows were run against it on 9 Nov 2026:

| What | Result | Tx |
|---|---|---|
| Deploy + init in one tx (hand-rolled Borsh signing) | ok | [DrGdkLs9…](https://testnet.nearblocks.io/txns/DrGdkLs9BErsQwj3gsw4ijaAyjMvANoaB4DE3TLkmWja) |
| Owner funds 1 NEAR | budget 1 NEAR | [87m9vLbT…](https://testnet.nearblocks.io/txns/87m9vLbTxc8zJ16Jcg8vVT7MVZb3NyCu6XPwcUC8gxsu) |
| Provider registers `sec-red-flags` at 0.01 NEAR; owner approves | approved | [GRM577DB…](https://testnet.nearblocks.io/txns/GRM577DB2usNqwJBEBupEPPvyGrM1vUFEbHuv2GfqRpF), [BMeLi3xm…](https://testnet.nearblocks.io/txns/BMeLi3xmTcsfmbpu6W2kmuovEyuNeGKs4oe7sa9XCoDP) |
| Agent tries to change the policy | `Smart contract panicked: only the owner` | [734KNdr4…](https://testnet.nearblocks.io/txns/734KNdr47zhg8U2GbvYCui6imzqJJDfGA9yxgpkE8WA5) |
| Agent buys NVDA filings: 402 → pay → redeem | receipt #0, provider served 1 filing | [B8Yw4ffs…](https://testnet.nearblocks.io/txns/B8Yw4ffsWzb519kwvQ87pwJf9SaUE4qTiS9GhoFCBWVm) |
| Replay receipt #0 | `409 already redeemed` | (off-chain) |
| Use receipt #0 for AAPL | `402 paid for a different request` | (off-chain) |
| Daily cap set to 0.03 NEAR; 3rd purchase of the day | `daily cap reached` | |
| Provider raises price to 0.02 | approval cleared; agent cannot pay until owner re-approves | |
| `scan --near` → JPM and XOM signals committed | `verify`: both `ok`, before the next open | [cLzdQY2b…](https://testnet.nearblocks.io/txns/cLzdQY2bfJGvSHSpGpqmXQ4wUq2UwZ7qgB5rxTBWCoU), [CK2zT6Ev…](https://testnet.nearblocks.io/txns/CK2zT6EvWikzEwHywg57sg1wndrm99mD51XFuQ4VpcPc) |
| JPM's target edited +1% in a copy of the ledger | `verify`: JPM `NOT COMMITTED`, XOM still `ok` | (off-chain) |

To upgrade the contract code while keeping its state: `axiom near deploy --no-init --as <contract account>`.

**Build note:** `contract/build.sh` passes `--cfg near`, the flag cargo-near normally sets. Without it, near-sdk 5 compiles contract panics into a bare `unreachable` trap and every error message is lost.

## Tests

```bash
cargo test                      # agent: Borsh layout, key round-trip, NEAR amounts, signal hash, receipt checks
cd contract && cargo test       # contract: caps, allowlist, price-change re-approval, hijack, budget, commitments
```

## Honest limits

- **Testnet only.** The payment flow is identical on mainnet, but nothing here has handled real money.
- **One provider, one service.** The registry is open (any account can register a service, paying its own storage), but the demo provider is run by the same team. The provider's data is public SEC EDGAR data; what is being sold is the fetching, filtering and red-flag tagging, not exclusive data.
- **Redeemed receipts are tracked in the provider's memory.** A restarted provider would accept an unexpired receipt again (at most 15 minutes old). The buyer is never charged twice for that.
- **Commitments prove *when* a signal existed, not that every signal was committed.** An agent could skip committing losers. `verify` lists ledger signals that were never committed, but someone who deleted rows from their own ledger could hide them. Commitments are numbered on-chain, so the count of commitments is public.
- **NEAR Intents is not used.** Payments are native NEAR transfers from the contract. Paying providers in other assets through Intents is the natural next step.

## Disclosures

- Contract built on [`near-sdk`](https://crates.io/crates/near-sdk) 5.x. Agent side uses `ed25519-dalek`, `sha2`, `bs58`, `base64`, `axum`, `reqwest`, `tokio`, `serde`.
- Code written with AI assistance (Claude Code).
- All code in this repository was written during the hackathon window (first commit 9 Nov 2026). The stock-research agent was built first; the NEAR layer is in the commits that follow it.
