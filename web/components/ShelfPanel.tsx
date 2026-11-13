"use client";
import { useEffect, useState } from "react";
import { explorerAccount } from "@/lib/config";
import { isMemeCommitment, memeWindowEnd, parseMemeLabel } from "@/lib/meme";
import { fmtNear, fmtUtc, nsToDate } from "@/lib/near";
import type { Market } from "@/lib/useMarket";

const short = (s: string) => `${s.slice(0, 10)}…${s.slice(-4)}`;

function left(ms: number): string {
  const h = Math.floor(ms / 3_600_000);
  return h >= 1 ? `${h}h left` : `${Math.max(1, Math.round(ms / 60_000))}m left`;
}

/**
 * The shelf: memecoin signals that are sealed on-chain and still inside their holding window, so they can be
 * bought now at a fixed price, with no bidding. Each shows only the teaser in its commitment label; the token
 * and prices are what the buyer pays for, and they publish here once the window ends.
 */
export default function ShelfPanel({ data }: { data: Market | null }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);

  const seller = data?.memeRecord?.committer ?? data?.record?.committer ?? null;
  const live = (data?.sellerCommitments ?? [])
    .filter(isMemeCommitment)
    .map((c) => ({ c, teaser: parseMemeLabel(c.label), ends: memeWindowEnd(c) }))
    .filter((r) => r.ends.getTime() > now);
  const svc = data?.services.find(([id, s]) => id === "meme-signals" && (!seller || s.provider === seller))?.[1];
  const outcomes = (data?.memeRecord?.outcomes ?? []).map((o) => o.outcome).filter((o) => o && o.ret !== null && o.status !== "open");
  const wins = outcomes.filter((o) => (o!.ret ?? 0) > 0).length;

  return (
    <div className="panel" id="shelf">
      <div className="panel-head">
        <div>
          <h2 className="h3">Live memecoin signals</h2>
          <p className="muted small">
            Ready-made picks, bought at a fixed price with no bidding. Each one is sealed on-chain the moment the agent finds a
            breakout that passed the rug-risk screen. The teaser below is public; the token, entry, stop and target are what you
            pay for. When the window ends the full signal publishes in the record below, where anyone can check it against
            its seal. For anything not on the shelf, post a job instead.
          </p>
        </div>
        {data && (
          <span className="meta-row">
            {svc ? <>{fmtNear(svc.price)} Ⓝ unlocks all live signals</> : "not on sale"}
          </span>
        )}
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Sealed</th>
              <th>Setup</th>
              <th>Rug risk</th>
              <th>Window</th>
              <th>Seal</th>
            </tr>
          </thead>
          <tbody>
            {live.map(({ c, teaser, ends }) => (
              <tr key={c.hash}>
                <td className="mono small">{fmtUtc(nsToDate(c.timestamp_ns))}</td>
                <td>
                  <b className="mono small">{teaser?.setup ?? "breakout"}</b>
                  <div className="muted small">token hidden until bought</div>
                </td>
                <td className="small">{teaser ? `${teaser.level} (${teaser.score})` : <span className="muted">passed screen</span>}</td>
                <td className="small">
                  {left(ends.getTime() - now)}
                  <div className="muted small">ends {fmtUtc(ends)}</div>
                </td>
                <td>
                  <span className="chip lock">sealed</span>
                  <div className="hash small">{short(c.hash)}</div>
                </td>
              </tr>
            ))}
            {data && live.length === 0 && (
              <tr>
                <td colSpan={5} className="empty">
                  Nothing on the shelf right now. The agent scans NEAR&apos;s active pools every hour and only shelves a breakout
                  that passes the rug-risk screen; the market has been quiet. A risk screen of any token is always available as a
                  job.
                </td>
              </tr>
            )}
            {!data && (
              <tr>
                <td colSpan={5} className="empty checking">
                  Reading the chain…
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      <p className="muted small">
        Seller{" "}
        {seller ? (
          <a className="mono" href={explorerAccount(seller)} target="_blank" rel="noreferrer">
            {seller}
          </a>
        ) : (
          "—"
        )}
        : {data?.memeRecord?.entries.length ?? 0} memecoin signal(s) published so far
        {outcomes.length > 0 && `, ${wins} of ${outcomes.length} closed in profit`}. An agent buys with{" "}
        <span className="mono">axiom meme shop --seller &lt;url&gt;</span>, which checks this record on-chain first and only
        pays if it passes. Breakout signals, not predictions: memecoins can go to zero.
      </p>
    </div>
  );
}
