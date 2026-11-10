import type { Metadata } from "next";
import { CONTRACT, SELLER } from "@/lib/config";

export const metadata: Metadata = {
  title: "Docs · Axiom",
  description: "How Axiom Pay works: wallets, services, receipts, commitments, the 402 flow, the CLI, the HTTP API and the contract.",
};

const toc: [string, [string, string][]][] = [
  [
    "Start",
    [
      ["overview", "Overview"],
      ["quickstart", "Quickstart"],
      ["concepts", "Concepts"],
    ],
  ],
  [
    "Guides",
    [
      ["buying", "Buying as an agent"],
      ["selling", "Selling a service"],
      ["research", "Axiom's research loop"],
      ["verification", "Verification spec"],
    ],
  ],
  [
    "Reference",
    [
      ["cli", "CLI"],
      ["http", "HTTP API"],
      ["contract", "Contract"],
      ["limits", "Limits"],
    ],
  ],
];

function Code({ children }: { children: string }) {
  return <pre className="code">{children.trim()}</pre>;
}

export default function Docs() {
  return (
    <div className="wrap docs-grid">
      <nav className="toc" aria-label="Docs contents">
        {toc.map(([group, items]) => (
          <div key={group} style={{ display: "contents" }}>
            <span className="eyebrow">{group}</span>
            {items.map(([id, label]) => (
              <a key={id} href={`#${id}`}>
                {label}
              </a>
            ))}
          </div>
        ))}
      </nav>

      <article className="prose">
        <h2 id="overview">Overview</h2>
        <p>
          <b>Axiom</b> is an AI research agent for US stocks. After each market close it scans a watchlist for a few named, backtested
          setups, gathers evidence (technicals, market regime, SEC filings, news) and writes a cited bull/bear report for every hit. It
          never places trades.
        </p>
        <p>
          <b>Axiom Pay</b> is the NEAR smart contract and tooling that turn that research into a product other agents can buy, and can
          check before buying:
        </p>
        <ul>
          <li>
            Axiom <b>commits</b> a SHA-256 of every signal on-chain before the next open.
          </li>
          <li>
            It <b>sells</b> fresh signals and reports per call, behind HTTP 402, settled on NEAR.
          </li>
          <li>
            Once a signal&apos;s entry is known it&apos;s <b>published</b> free, so buyers can <b>verify</b> the whole record against the
            chain.
          </li>
          <li>
            Buyer agents spend from <b>wallets</b> their humans control: budget, per-call cap, daily cap, approved sellers at approved
            prices.
          </li>
        </ul>
        <div className="callout">
          Live deployment: contract <code>{CONTRACT}</code> on NEAR testnet, seller <code>{SELLER}</code>. The{" "}
          <a href="../market/">market page</a> reads both directly from the chain.
        </div>

        <h2 id="quickstart">Quickstart</h2>
        <p>
          You need Rust. No near-cli: the <code>axiom</code> binary signs NEAR transactions itself. Steps 1–4 and 6 need no market-data
          keys; step 5 (a real scan) needs the free Alpaca keys described in the README.
        </p>
        <Code>{`
cargo build --release && ./contract/build.sh
A=./target/release/axiom; S=$RANDOM

# 1. Testnet accounts from the faucet (keys saved to ~/.near-credentials/testnet/)
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

# 4. Run the sellers (two terminals)
$A near serve --port 8787 --as ax-axiom-$S.testnet
SEC_USER_AGENT="axiom-demo you@example.com" $A near serve --port 8788 --as ax-sec-$S.testnet

# 5. Axiom researches: buys SEC evidence, writes reports, commits each signal
NEAR_ACCOUNT_ID=ax-axiom-$S.testnet AXIOM_SEC_PROVIDER_URL=http://127.0.0.1:8788 \\
  $A scan --near --no-llm --no-telegram

# 6. The buyer agent shops
$A near shop --as ax-bot-$S.testnet                              # refuses: no verified history yet
$A near shop --min-verified 0 --reports --as ax-bot-$S.testnet   # buys signals and reports
`}</Code>
        <p>
          Expected: the first <code>shop</code> prints <code>DECISION: not buying. Only 0 signal(s) verified on-chain</code>. The second
          pays twice or more, marks each signal <code>committed on-chain</code>, and saves reports under <code>purchased/&lt;date&gt;/</code>.
          After the next open, <code>axiom near check-seller</code> shows those signals as verified, and the default policy buys.
        </p>

        <h2 id="concepts">Concepts</h2>
        <h3>Wallet</h3>
        <p>
          Opened by a human <b>owner</b> for one <b>agent</b> account. Holds a budget (deposited NEAR) and a policy: <code>max_per_call</code>{" "}
          and <code>daily_cap</code>. An agent spends from exactly one wallet; the day rolls over at 00:00 UTC.
        </p>
        <h3>Service</h3>
        <p>
          A priced thing a <b>seller</b> sells, identified by a <code>service_id</code>. Payment goes straight to the seller&apos;s account.
          Only the seller who registered an id can reprice it.
        </p>
        <h3>Approval</h3>
        <p>
          An owner approves a service <b>at a price</b>. If the seller changes the price, the approval no longer matches and the agent
          can&apos;t pay until the owner approves the new price.
        </p>
        <h3>Receipt</h3>
        <p>
          Created by every payment: service, seller, payer, owner, amount, time and the <code>request_hash</code>, a SHA-256 of the exact
          request being paid for. The seller serves only that request, once, within 15 minutes.
        </p>
        <h3>Commitment</h3>
        <p>
          A SHA-256 recorded by any account, keyed by <code>(committer, hash)</code>. Its block time proves the underlying record existed
          then. Nobody can commit on another account&apos;s behalf.
        </p>

        <h2 id="buying">Buying as an agent</h2>
        <p>Purchases follow an HTTP 402 handshake. The buyer never trusts the seller&apos;s numbers: it recomputes the request hash itself.</p>
        <ol>
          <li>
            <code>GET /v1/&lt;service&gt;?…</code> without a receipt. The seller answers <code>402</code> with its terms.
          </li>
          <li>
            Recompute <code>request_hash</code>; check the contract matches yours, and that your owner approved this service at its current
            price (<code>get_wallet_of_agent</code>, <code>get_approval</code>).
          </li>
          <li>
            Call <code>pay(service_id, request_hash, memo)</code>. The contract enforces approval, caps and budget, pays the seller and
            returns a receipt.
          </li>
          <li>
            Repeat the request with <code>&amp;receipt=&lt;id&gt;</code>. Retry on 5xx: the seller releases the receipt when its own
            upstream fails.
          </li>
        </ol>
        <Code>{`
HTTP/1.1 402 Payment Required
{
  "error": "payment required",
  "network": "testnet",
  "contract": "${CONTRACT}",
  "method": "pay",
  "service_id": "axiom-signals",
  "price": "20000000000000000000000",
  "seller": "${SELLER}",
  "request": "axiom-pay/v1|axiom-signals|2026-11-09",
  "request_hash": "1b9fe78e5d55…",
  "track_record": "/v1/track-record"
}
`}</Code>
        <p>
          <code>axiom near shop</code> does all of this, after first verifying the seller&apos;s record (see{" "}
          <a href="#verification">verification</a>) against its policy flags <code>--min-verified</code> and{" "}
          <code>--max-unverified-share</code>.
        </p>

        <h2 id="selling">Selling a service</h2>
        <ol>
          <li>
            <code>axiom near register --service &lt;id&gt; --price &lt;NEAR&gt; --as &lt;seller&gt;</code>. Attaches 0.01 NEAR for storage;
            the unused part is refunded.
          </li>
          <li>
            <code>axiom near serve --as &lt;seller&gt;</code> serves every service that account registered.
          </li>
          <li>
            A paid request is served only if the receipt names this seller and service, its <code>request_hash</code> matches this exact
            request, it is under 15 minutes old, and it hasn&apos;t been redeemed before.
          </li>
          <li>The server checks it has something to sell before quoting a price, so buyers are never charged for an empty answer.</li>
        </ol>

        <h2 id="research">Axiom&apos;s research loop</h2>
        <p>
          <code>axiom scan --near</code> runs after the close: it scans the watchlist, buys SEC evidence from an evidence seller when{" "}
          <code>AXIOM_SEC_PROVIDER_URL</code> is set, writes a cited report per hit, logs a paper trade, and commits each signal. The
          on-chain label names only the session (<code>axiom-signal/v1 2026-11-09</code>), never the symbol, so nothing being sold leaks
          early. <code>axiom near export-record</code> writes the free record the website reads.
        </p>
        <div className="callout">
          Code computes, the model explains: entries, stops, targets and statistics come from deterministic Rust. The optional LLM only
          writes the bull and bear narrative, citing evidence IDs.
        </div>

        <h2 id="verification">Verification spec</h2>
        <p>A signal&apos;s canonical line, with prices at exactly four decimals (ties rounded to even, as Rust formats them):</p>
        <Code>{`axiom-signal/v1|<SYMBOL>|<setup>|<YYYY-MM-DD>|<entry>|<stop>|<target>|<max_hold>
axiom-signal/v1|JPM|trend-pullback|2026-11-09|339.6900|323.9929|371.0841|20`}</Code>
        <p>To verify an entry from a seller&apos;s record:</p>
        <ol>
          <li>Rebuild the line from the entry&apos;s fields and take its SHA-256 (lowercase hex).</li>
          <li>
            Call <code>find_commitment(committer = seller, hash)</code>.
          </li>
          <li>
            Compare the commitment&apos;s <code>timestamp_ns</code> with the deadline: 13:30 UTC on the fill day, or on the next weekday
            after the session if it hasn&apos;t filled. That is the earliest moment the entry price could be known.
          </li>
        </ol>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Result</th>
                <th>Meaning</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>
                  <span className="chip ok">✓ before open</span>
                </td>
                <td>Counts toward the track record.</td>
              </tr>
              <tr>
                <td>
                  <span className="chip late">late</span>
                </td>
                <td>Committed after the entry was knowable. Doesn&apos;t count.</td>
              </tr>
              <tr>
                <td>
                  <span className="chip bad">not on-chain</span>
                </td>
                <td>Never committed, or a field was changed after committing.</td>
              </tr>
            </tbody>
          </table>
        </div>

        <h2 id="cli">CLI</h2>
        <p>
          All commands take <code>--as &lt;account&gt;</code> (key from <code>~/.near-credentials/&lt;network&gt;/</code>) and read{" "}
          <code>AXIOM_PAY_CONTRACT</code>, <code>NEAR_NETWORK</code> (default testnet) and <code>NEAR_RPC_URL</code> (default FastNEAR).
        </p>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Command</th>
                <th>Who</th>
                <th>Does</th>
              </tr>
            </thead>
            <tbody>
              {[
                ["create-account <id>", "anyone", "Funded testnet account from the faucet; saves its key"],
                ["deploy [--no-init]", "deployer", "Deploys the contract (and initialises it) in one transaction"],
                ["status", "anyone", "Services, sales, and your wallet if you have one"],
                ["open-wallet --agent --budget", "owner", "Opens a wallet with caps; deposit pays storage, rest is budget"],
                ["fund <NEAR> · policy · approve · revoke", "owner", "Manage budget, caps and approved services"],
                ["register --service --price", "seller", "Register or reprice a service"],
                ["serve [--port]", "seller", "HTTP server for every service you registered"],
                ["shop [--min-verified] [--reports]", "buyer agent", "Verify the seller, decide, buy signals (and reports)"],
                ["buy <SYMBOL>", "agent", "Buy SEC filings from an evidence seller"],
                ["check-seller", "anyone", "Verify a seller's published record against the chain"],
                ["commit-signals · verify · export-record", "seller", "Commit, check and publish your own ledger"],
                ["receipts [--last]", "anyone", "Latest payments"],
              ].map(([c, w, d]) => (
                <tr key={c}>
                  <td>
                    <code>axiom near {c}</code>
                  </td>
                  <td className="small">{w}</td>
                  <td className="small">{d}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <h2 id="http">HTTP API</h2>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Endpoint</th>
                <th>Price</th>
                <th>Returns</th>
              </tr>
            </thead>
            <tbody>
              {[
                ["GET /", "free", "Seller account, contract, services, latest signal date"],
                ["GET /v1/track-record", "free", "Signals whose entry is known, with all fields needed to re-hash them"],
                ["GET /v1/axiom-signals?date=", "0.02 Ⓝ", "That session's signals: entry, stop, target, R:R, commitment hash"],
                ["GET /v1/axiom-report?symbol=&date=", "0.05 Ⓝ", "The full cited Markdown report(s)"],
                ["GET /v1/sec-red-flags?symbol=&since=", "0.01 Ⓝ", "SEC filings with 8-K item codes (evidence seller)"],
              ].map(([e, p, r]) => (
                <tr key={e}>
                  <td>
                    <code>{e}</code>
                  </td>
                  <td className="small mono">{p}</td>
                  <td className="small">{r}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p>
          Paid endpoints answer <code>402</code> with terms, then <code>200</code> with <code>&amp;receipt=&lt;id&gt;</code>. Errors:{" "}
          <code>409</code> receipt already redeemed, <code>402</code> receipt rejected (wrong request, seller, service, or expired),{" "}
          <code>404</code> nothing to sell for that request.
        </p>

        <h2 id="contract">Contract</h2>
        <p>
          Rust, <code>near-sdk</code> 5. Source: <code>contract/src/lib.rs</code>, 16 unit tests. Amounts are yoctoNEAR strings.
        </p>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Method</th>
                <th>Caller</th>
                <th>Effect</th>
              </tr>
            </thead>
            <tbody>
              {[
                ["open_wallet(agent, max_per_call, daily_cap) 💰", "owner", "Creates the wallet; deposit minus storage becomes budget"],
                ["deposit() 💰 · withdraw(amount)", "owner", "Adds to or withdraws from the budget"],
                ["set_policy(max_per_call, daily_cap) · set_agent(agent)", "owner", "Changes caps or the agent"],
                ["approve_service(service_id, price) · revoke_service", "owner", "Allowlist at the current price"],
                ["register_service(service_id, price, description) 💰", "seller", "Creates or reprices; pays own storage"],
                ["pay(service_id, request_hash, memo)", "agent", "Checks approval, caps, budget; pays seller; returns Receipt"],
                ["commit(hash, label) 💰", "anyone", "Records (caller, hash) with block time; pays own storage"],
                ["get_wallet · get_wallet_of_agent · get_approval", "view", "Wallet state and approvals"],
                ["get_service · list_services", "view", "Services with price, sales and earnings"],
                ["get_receipt · get_receipts · receipt_count", "view", "Payments"],
                ["find_commitment(committer, hash) · get_commitments · commitment_count", "view", "Commitments"],
              ].map(([m, c, e]) => (
                <tr key={m}>
                  <td>
                    <code>{m}</code>
                  </td>
                  <td className="small">{c}</td>
                  <td className="small">{e}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p className="small muted">💰 attaches NEAR.</p>

        <h2 id="limits">Limits</h2>
        <ul>
          <li>Testnet only; nothing here has handled real money.</li>
          <li>Payments are native NEAR. Paying in USDC through NEAR Intents is the next step (Intents is mainnet-only).</li>
          <li>Returns are the seller&apos;s own paper-trade accounting; timestamps and contents are what the chain proves.</li>
          <li>A signal that was never committed leaves no trace; committed ones are numbered and public.</li>
          <li>Redeemed receipts are tracked in the seller&apos;s memory; a restarted seller could honour an unexpired receipt once more.</li>
          <li>Research only, not investment advice. Axiom never places trades.</li>
        </ul>
      </article>
    </div>
  );
}
