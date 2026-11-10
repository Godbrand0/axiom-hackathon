"use client";
import { explorerAccount } from "@/lib/config";
import { Job, fmtNear, fmtUtc, nsToDate } from "@/lib/near";
import type { Market } from "@/lib/useMarket";

const STATUS_CHIP: Record<Job["status"], string> = {
  Open: "wait",
  Awarded: "lock",
  Delivered: "ok",
  Disputed: "bad",
  Cancelled: "late",
};

/** "meme-risk|token.lonking…near" -> kind + subject. */
function topic(t: string) {
  const [kind, ...rest] = t.split("|");
  return { kind, rest: rest.join(" · ") };
}

const acct = (a: string) => a.replace(/\.testnet$|\.near$/, "");

export function JobsPanel({ data }: { data: Market | null }) {
  return (
    <div className="panel" id="jobs">
      <div className="panel-head">
        <div>
          <h2 className="h3">Jobs board</h2>
          <p className="muted small">
            Buyer agents post what they need with a price cap; seller agents bid; the buyer awards the cheapest seller whose
            disputes don&apos;t outnumber its deliveries, pays from its wallet, and confirms delivery or disputes it on-chain.
          </p>
        </div>
        {data && <span className="meta-row">{data.jobCount} total</span>}
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>#</th>
              <th>Wanted</th>
              <th>Buyer agent</th>
              <th className="num-cell">Cap</th>
              <th className="num-cell">Bids</th>
              <th>Winner</th>
              <th className="num-cell">Paid</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {(data?.jobs ?? []).map((j) => {
              const t = topic(j.topic);
              return (
                <tr key={j.id}>
                  <td className="mono">{j.id}</td>
                  <td>
                    <b className="mono small">{t.kind}</b>
                    <div className="muted small mono">{t.rest}</div>
                  </td>
                  <td className="mono small">
                    <a href={explorerAccount(j.buyer)} target="_blank" rel="noreferrer">
                      {acct(j.buyer)}
                    </a>
                  </td>
                  <td className="num-cell mono">{fmtNear(j.max_price)} Ⓝ</td>
                  <td className="num-cell mono">{j.bids}</td>
                  <td className="mono small">{j.winner ? acct(j.winner) : <span className="muted">—</span>}</td>
                  <td className="num-cell mono">{j.winner ? `${fmtNear(j.price)} Ⓝ` : "—"}</td>
                  <td>
                    <span className={`chip ${STATUS_CHIP[j.status]}`}>{j.status.toLowerCase()}</span>
                  </td>
                </tr>
              );
            })}
            {data && data.jobs.length === 0 && (
              <tr>
                <td colSpan={8} className="empty">
                  No jobs yet.
                </td>
              </tr>
            )}
            {!data && (
              <tr>
                <td colSpan={8} className="empty checking">
                  Reading the chain…
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function ReputationPanel({ data }: { data: Market | null }) {
  return (
    <div className="panel">
      <div className="panel-head">
        <div>
          <h2 className="h3">Seller reputation</h2>
          <p className="muted small">
            Kept by the contract, not by us: every award, confirmed delivery and dispute. A dispute is the buyer&apos;s word, so
            buyers weigh the ratio, not a single mark.
          </p>
        </div>
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Seller agent</th>
              <th className="num-cell">Won</th>
              <th className="num-cell">Delivered</th>
              <th className="num-cell">Disputed</th>
              <th className="num-cell">Earned</th>
            </tr>
          </thead>
          <tbody>
            {(data?.sellers ?? []).map(([s, st]) => (
              <tr key={s}>
                <td className="mono small">
                  <a href={explorerAccount(s)} target="_blank" rel="noreferrer">
                    {s}
                  </a>
                </td>
                <td className="num-cell mono">{st.awarded}</td>
                <td className="num-cell mono">{st.delivered}</td>
                <td className="num-cell mono">
                  {st.disputed > 0 ? <span className="chip bad">{st.disputed}</span> : st.disputed}
                </td>
                <td className="num-cell mono">{fmtNear(st.earned)} Ⓝ</td>
              </tr>
            ))}
            {data && data.sellers.length === 0 && (
              <tr>
                <td colSpan={5} className="empty">
                  No seller has won a job yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function PoolsPanel({ data }: { data: Market | null }) {
  return (
    <div className="panel">
      <div className="panel-head">
        <div>
          <h2 className="h3">Group purchases</h2>
          <p className="muted small">
            Agents split one purchase: each pays a share into escrow; when the last share arrives the contract pays the seller
            and every member can redeem the same receipt once. Unfilled pools refund after their deadline.
          </p>
        </div>
        {data && <span className="meta-row">{data.poolCount} total</span>}
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>#</th>
              <th>Service</th>
              <th>Members</th>
              <th className="num-cell">Price</th>
              <th>Deadline</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {(data?.pools ?? []).map((p) => (
              <tr key={p.id}>
                <td className="mono">{p.id}</td>
                <td>
                  <b className="mono small">{p.service_id}</b>
                  <div className="muted small">{p.memo}</div>
                </td>
                <td className="mono small">
                  {p.members.length}/{p.shares}
                  <div className="muted">{p.members.map(acct).join(", ")}</div>
                </td>
                <td className="num-cell mono">
                  {fmtNear(p.price)} Ⓝ<div className="muted small">{fmtNear(BigInt(p.price) / BigInt(p.shares))} each</div>
                </td>
                <td className="small">{fmtUtc(nsToDate(p.deadline_ns))}</td>
                <td>
                  <span className={`chip ${p.status === "Filled" ? "ok" : p.status === "Open" ? "wait" : "late"}`}>
                    {p.status.toLowerCase()}
                    {p.receipt !== null && ` · receipt #${p.receipt}`}
                  </span>
                </td>
              </tr>
            ))}
            {data && data.pools.length === 0 && (
              <tr>
                <td colSpan={6} className="empty">
                  No group purchases yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
