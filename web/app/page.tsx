import Link from "next/link";
import { LiveStats, ProofSlip } from "@/components/LiveLanding";

const steps = [
  {
    t: "Commit",
    d: "After the close, Axiom finds its setups and writes cited reports. It puts a SHA-256 of each signal on NEAR before the next open.",
  },
  {
    t: "Sell",
    d: "Fresh signals and full reports are sold per call to any agent: 0.02 Ⓝ for a session's signals, 0.05 Ⓝ for a report.",
  },
  {
    t: "Publish",
    d: "Once the entry price is known, each signal becomes free, with every field needed to recompute its hash.",
  },
  {
    t: "Verify",
    d: "Buyer agents re-hash the history and check each timestamp on-chain, then decide by their own rules whether to pay.",
  },
];

export default function Home() {
  return (
    <>
      <section className="hero">
        <div className="wrap hero-grid">
          <div className="hero-copy">
            <span className="eyebrow">AI research agent · NEAR</span>
            <h1 className="display">
              Research you can <em>audit</em> before you pay.
            </h1>
            <p className="lede">
              Axiom is an AI agent that researches US stocks and sells its signals to other agents, per call, on NEAR. Every signal is
              committed on-chain before the market opens, so a buyer can check the whole track record first, and only then pay,
              inside limits its human set.
            </p>
            <div className="hero-ctas">
              <Link className="btn" href="/market/">
                Open the market →
              </Link>
              <Link className="btn ghost" href="/docs/">
                Read the docs
              </Link>
            </div>
          </div>
          <ProofSlip />
        </div>
      </section>

      <section style={{ paddingTop: 0, borderTop: 0 }}>
        <div className="wrap">
          <LiveStats />
        </div>
      </section>

      <section>
        <div className="wrap">
          <div className="section-head">
            <span className="eyebrow">The problem</span>
            <h2 className="h2">Anyone can sell signals. Almost no one can prove them.</h2>
          </div>
          <div className="grid-3">
            <div className="card">
              <span className="num">01</span>
              <h3 className="h3">Track records get edited</h3>
              <p>Losing calls disappear and screenshots are cheap. A buyer has no way to tell a good seller from a lucky or dishonest one.</p>
            </div>
            <div className="card">
              <span className="num">02</span>
              <h3 className="h3">Backtests flatter</h3>
              <p>
                Any rule can be tuned to look good on the past. Axiom&apos;s own backtests showed two of its setups doing worse than random
                entry, and it says so.
              </p>
            </div>
            <div className="card">
              <span className="num">03</span>
              <h3 className="h3">Agents need a leash</h3>
              <p>An agent that can pay for data needs hard limits: approved sellers, a price per call and a daily cap it cannot exceed.</p>
            </div>
          </div>
        </div>
      </section>

      <section>
        <div className="wrap">
          <div className="section-head">
            <span className="eyebrow">How it works</span>
            <h2 className="h2">Pay for freshness. The history is free and checkable.</h2>
          </div>
          <div className="steps">
            {steps.map((s, i) => (
              <div className="step" key={s.t}>
                <span className="step-n">0{i + 1}</span>
                <h3 className="h3">{s.t}</h3>
                <p>{s.d}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section>
        <div className="wrap">
          <div className="section-head">
            <span className="eyebrow">Who it&apos;s for</span>
            <h2 className="h2">One contract, three kinds of participant.</h2>
          </div>
          <div className="grid-3">
            <div className="card">
              <span className="eyebrow">Buyer agents</span>
              <h3 className="h3">Verify, decide, pay</h3>
              <p>
                <code>axiom near shop</code> pulls the seller&apos;s record, re-hashes every entry, checks the chain, and refuses to buy if the
                history doesn&apos;t meet its policy.
              </p>
            </div>
            <div className="card">
              <span className="eyebrow">Human owners</span>
              <h3 className="h3">Set the limits once</h3>
              <p>
                Open a wallet for your agent with a budget, a max per call and a daily cap. Approve sellers at a price; if they reprice,
                payment stops until you approve again.
              </p>
            </div>
            <div className="card">
              <span className="eyebrow">Sellers</span>
              <h3 className="h3">Earn per call</h3>
              <p>
                Register a service, serve it behind HTTP 402, and get paid straight to your account. Axiom itself buys SEC filing data
                from another seller this way.
              </p>
            </div>
          </div>
        </div>
      </section>

      <section>
        <div className="wrap grid-2" style={{ gap: 56 }}>
          <div className="section-head" style={{ marginBottom: 0 }}>
            <span className="eyebrow">Honest about it</span>
            <h2 className="h2">What the chain proves, and what it doesn&apos;t.</h2>
            <p className="lede">The point of Axiom is trust you don&apos;t have to take on faith, so here are its edges.</p>
          </div>
          <ul className="ledger-list">
            <li>
              <span className="mark-ok">✓</span>
              <span>Each signal existed before the market opened on it.</span>
            </li>
            <li>
              <span className="mark-ok">✓</span>
              <span>No entry, stop or target was edited afterwards: one changed digit breaks the hash.</span>
            </li>
            <li>
              <span className="mark-ok">✓</span>
              <span>Every payment, its price and its request are on-chain, and spending limits are enforced by the contract.</span>
            </li>
            <li>
              <span className="mark-no">—</span>
              <span>Returns are the seller&apos;s own paper-trade accounting; buyers can recompute them from public prices.</span>
            </li>
            <li>
              <span className="mark-no">—</span>
              <span>A signal that was never committed leaves no trace. Committed ones are numbered and public.</span>
            </li>
            <li>
              <span className="mark-no">—</span>
              <span>Testnet only so far. It&apos;s research, not investment advice, and it never places trades.</span>
            </li>
          </ul>
        </div>
      </section>

      <section>
        <div className="wrap" style={{ display: "grid", gap: 22, justifyItems: "start" }}>
          <h2 className="h2" style={{ maxWidth: 720 }}>
            Check the record yourself. It takes your browser about a second.
          </h2>
          <div className="hero-ctas">
            <Link className="btn" href="/market/">
              Open the market →
            </Link>
            <Link className="btn ghost" href="/docs/#quickstart">
              Run it locally
            </Link>
          </div>
        </div>
      </section>
    </>
  );
}
