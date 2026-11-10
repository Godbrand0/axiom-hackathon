"use client";
import { useEffect, useMemo, useState } from "react";
import { explorerAccount } from "@/lib/config";
import { MemeCheck, MemeSignal, fmtPrice, isMemeCommitment, verifyMeme } from "@/lib/meme";
import { Commitment, fmtUtc, nsToDate } from "@/lib/near";
import type { Market } from "@/lib/useMarket";

type Row = { s: MemeSignal; hash?: string; check?: MemeCheck; c?: Commitment | null; err?: boolean };

const short = (s: string, n = 12) => (s.length > n * 2 ? `${s.slice(0, n)}…${s.slice(-4)}` : s);
const pct = (x: number) => `${x > 0 ? "+" : ""}${(x * 100).toFixed(1)}%`;

function Chip({ row }: { row: Row }) {
  if (row.err) return <span className="chip bad">RPC error</span>;
  if (!row.check) return <span className="chip wait checking">checking…</span>;
  if (row.check === "ontime") return <span className="chip ok">✓ committed first</span>;
  if (row.check === "late") return <span className="chip late">time mismatch</span>;
  return <span className="chip bad">not on-chain</span>;
}

/** Memecoin breakout signals: published ones re-hashed in the browser, sealed ones shown from the chain. */
export default function MemePanel({ data }: { data: Market | null }) {
  const rec = data?.memeRecord ?? null;
  const [rows, setRows] = useState<Row[]>([]);

  useEffect(() => {
    if (!rec) return;
    let cancelled = false;
    setRows(rec.entries.map((s) => ({ s })));
    (async () => {
      for (let i = 0; i < rec.entries.length; i++) {
        const s = rec.entries[i];
        try {
          const r = await verifyMeme(s, rec.committer);
          if (!cancelled) setRows((p) => p.map((row, j) => (j === i ? { s, ...r } : row)));
        } catch {
          if (!cancelled) setRows((p) => p.map((row, j) => (j === i ? { s, err: true } : row)));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [rec]);

  const outcomes = useMemo(() => new Map((rec?.outcomes ?? []).map((o) => [o.hash, o.outcome])), [rec]);
  const published = new Set(rows.map((r) => r.hash).filter(Boolean));
  const done = rows.every((r) => r.check || r.err);
  const sealed = (data?.sellerCommitments ?? []).filter((c) => isMemeCommitment(c) && done && !published.has(c.hash));
  const closed = rows.map((r) => (r.hash ? outcomes.get(r.hash) : null)).filter((o) => o && o.ret !== null && o.status !== "open");
  const wins = closed.filter((o) => (o!.ret ?? 0) > 0).length;
  const mean = closed.length ? closed.reduce((a, o) => a + (o!.ret ?? 0), 0) / closed.length : null;

  return (
    <div className="panel" id="memecoins">
      <div className="panel-head">
        <div>
          <h2 className="h3">Memecoin signals</h2>
          <p className="muted small">
            Breakouts on NEAR memecoins, each committed on-chain before it is sold and scored only from the first full hour after
            the commitment. High-risk tokens never get a signal. Signals publish here when their 72-hour window ends; until then
            they are sealed and for sale (<span className="mono">meme-signals</span>, 0.01 Ⓝ).
          </p>
        </div>
        <div className="meta-row">
          <span>
            <b>{rows.filter((r) => r.check === "ontime").length}</b> verified
          </span>
          <span>
            <b>{sealed.length}</b> sealed
          </span>
          <span>
            <b>{closed.length}</b> closed · {wins} wins{mean !== null && ` · mean ${pct(mean)}`}
          </span>
        </div>
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Signal (UTC)</th>
              <th>Token</th>
              <th className="num-cell">Entry</th>
              <th className="num-cell">Stop</th>
              <th className="num-cell">Target</th>
              <th>Rug risk</th>
              <th>Check</th>
              <th className="num-cell">Result</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => {
              const o = r.hash ? outcomes.get(r.hash) : null;
              return (
                <tr key={`${r.s.token}-${r.s.signal_time}`}>
                  <td className="mono small">{fmtUtc(new Date(r.s.signal_time))}</td>
                  <td>
                    <b>{r.s.symbol}</b>{" "}
                    <a className="muted small mono" href={explorerAccount(r.s.token)} target="_blank" rel="noreferrer">
                      {short(r.s.token, 14)}
                    </a>
                    {r.hash && <div className="hash small">{short(r.hash)}</div>}
                  </td>
                  <td className="num-cell mono">{fmtPrice(r.s.entry)}</td>
                  <td className="num-cell mono">{fmtPrice(r.s.stop)}</td>
                  <td className="num-cell mono">{fmtPrice(r.s.target)}</td>
                  <td className="small">
                    {r.s.risk.level} ({r.s.risk.score})
                  </td>
                  <td>
                    <Chip row={r} />
                  </td>
                  <td className="num-cell mono">
                    {o && o.ret !== null ? (
                      <>
                        {pct(o.ret)} <span className="muted small">{o.status}</span>
                      </>
                    ) : (
                      <span className="muted">{o?.status ?? "—"}</span>
                    )}
                  </td>
                </tr>
              );
            })}
            {sealed.map((c) => (
              <tr key={c.hash}>
                <td className="mono small">{fmtUtc(nsToDate(c.timestamp_ns))}</td>
                <td>
                  <span className="muted">Sealed signal</span>
                  <div className="hash small">{short(c.hash)}</div>
                </td>
                <td className="num-cell muted" colSpan={4}>
                  committed at block {c.block_height.toLocaleString()}; contents for sale until its window ends
                </td>
                <td>
                  <span className="chip lock">sealed</span>
                </td>
                <td className="num-cell muted">—</td>
              </tr>
            ))}
            {data && rows.length === 0 && sealed.length === 0 && (
              <tr>
                <td colSpan={8} className="empty">
                  No memecoin breakout has fired yet. The agent scans NEAR&apos;s active pools every hour; risk screens are on sale
                  meanwhile (<span className="mono">meme-risk</span>, 0.005 Ⓝ).
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
      <p className="muted small">
        Your browser re-hashes each published signal and checks the seller&apos;s commitment and its time on-chain. Results come
        from the seller&apos;s export; recompute them from public candles with{" "}
        <span className="mono">axiom meme verify --seller &lt;url&gt;</span>, which also counts commitments never revealed
        {rec?.unrevealed ? ` (currently ${rec.unrevealed})` : ""}.
      </p>
    </div>
  );
}
