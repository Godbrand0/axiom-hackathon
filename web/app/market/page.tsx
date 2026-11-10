"use client";
import { useEffect, useMemo, useState } from "react";
import { CONTRACT, NETWORK, SELLER, explorerAccount } from "@/lib/config";
import {
  Check,
  Commitment,
  RecordEntry,
  entryKnownAt,
  fmtDuration,
  fmtNear,
  fmtUtc,
  nsToDate,
  sha256Hex,
  verifyEntry,
  view,
} from "@/lib/near";
import { labelDate, useMarket } from "@/lib/useMarket";
import { isMemeCommitment } from "@/lib/meme";
import MemePanel from "@/components/MemePanel";
import { JobsPanel, PoolsPanel, ReputationPanel } from "@/components/JobsPanel";

type Row = { e: RecordEntry; hash?: string; check?: Check; c?: Commitment | null; err?: string };

function CheckChip({ check }: { check?: Check }) {
  if (!check) return <span className="chip wait checking">checking…</span>;
  if (check === "ontime") return <span className="chip ok">✓ before open</span>;
  if (check === "late") return <span className="chip late">late</span>;
  return <span className="chip bad">not on-chain</span>;
}

function short(s: string, n = 10) {
  return s.length > n * 2 ? `${s.slice(0, n)}…${s.slice(-4)}` : s;
}

export default function MarketPage() {
  const { data, error, loading, refresh } = useMarket();
  const [rows, setRows] = useState<Row[]>([]);

  // Verify every published entry in the browser: re-hash, then look up the seller's commitment.
  useEffect(() => {
    const rec = data?.record;
    if (!rec) return;
    let cancelled = false;
    setRows(rec.entries.map((e) => ({ e })));
    (async () => {
      for (let i = 0; i < rec.entries.length; i++) {
        const e = rec.entries[i];
        try {
          const r = await verifyEntry(e, rec.committer);
          if (cancelled) return;
          setRows((prev) => prev.map((row, j) => (j === i ? { e, hash: r.hash, check: r.check, c: r.commitment } : row)));
        } catch (err) {
          if (cancelled) return;
          setRows((prev) => prev.map((row, j) => (j === i ? { e, err: String(err) } : row)));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [data?.record]);

  const publishedHashes = useMemo(() => new Set(rows.map((r) => r.hash).filter(Boolean)), [rows]);
  const allChecked = rows.every((r) => r.check || r.err);
  const sealed = (data?.sellerCommitments ?? []).filter((c) => allChecked && !isMemeCommitment(c) && !publishedHashes.has(c.hash));

  const onTime = rows.filter((r) => r.check === "ontime");
  const closed = onTime.filter((r) => r.e.ret !== null);
  const wins = closed.filter((r) => (r.e.ret ?? 0) > 0).length;
  const mean = closed.length ? closed.reduce((s, r) => s + (r.e.ret ?? 0), 0) / closed.length : null;
  const seller = data?.record?.committer ?? SELLER;

  return (
    <div className="wrap">
      <div className="page-head">
        <span className="eyebrow">Live · NEAR {NETWORK}</span>
        <h1 className="h2">The market</h1>
        <p className="lede" style={{ margin: 0 }}>
          Everything below is read from the blockchain by your browser. The track record is re-hashed here and checked against the
          seller&apos;s on-chain commitments; nothing is taken from our server on trust.
        </p>
        <div className="meta-row">
          <span>
            Contract{" "}
            <a className="mono" href={explorerAccount(CONTRACT)} target="_blank" rel="noreferrer">
              <b>{CONTRACT}</b>
            </a>
          </span>
          <span>
            Seller{" "}
            <a className="mono" href={explorerAccount(seller)} target="_blank" rel="noreferrer">
              <b>{seller}</b>
            </a>
          </span>
          {data && <span>Read at {fmtUtc(data.loadedAt)}</span>}
          <button className="btn ghost sm" onClick={() => refresh(true)} disabled={loading}>
            {loading ? "Reading…" : "Refresh"}
          </button>
        </div>
        {error && <div className="notice">Could not reach NEAR: {error}. Try Refresh in a moment.</div>}
      </div>

      {/* ---------------- track record ---------------- */}
      <div className="panel">
        <div className="panel-head">
          <div>
            <h2 className="h3">Stock signals: Axiom&apos;s track record</h2>
            <p className="muted small">
              Published signals (entry already known) with the check your browser just ran, followed by sealed signals: committed
              on-chain, contents for sale until the open.
            </p>
          </div>
          <div className="meta-row">
            <span>
              <b>{onTime.length}</b> verified before open
            </span>
            <span>
              <b>{rows.filter((r) => r.check === "late" || r.check === "missing").length}</b> late or missing
            </span>
            <span>
              <b>{closed.length}</b> closed · {wins} wins{mean !== null && ` · mean ${(mean * 100).toFixed(2)}%`}
            </span>
          </div>
        </div>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Session</th>
                <th>Signal</th>
                <th className="num-cell">Entry</th>
                <th className="num-cell">Stop</th>
                <th className="num-cell">Target</th>
                <th>Committed</th>
                <th>Check</th>
                <th className="num-cell">Result</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const lead = r.c ? entryKnownAt(r.e.as_of, r.e.fill_date).getTime() - nsToDate(r.c.timestamp_ns).getTime() : null;
                return (
                  <tr key={`${r.e.symbol}-${r.e.setup}-${r.e.as_of}`}>
                    <td className="mono">{r.e.as_of}</td>
                    <td>
                      <b>{r.e.symbol}</b> <span className="muted small">{r.e.setup}</span>
                      {r.hash && <div className="hash small">{short(r.hash, 12)}</div>}
                    </td>
                    <td className="num-cell mono">{r.e.entry.toFixed(2)}</td>
                    <td className="num-cell mono">{r.e.stop.toFixed(2)}</td>
                    <td className="num-cell mono">{r.e.target.toFixed(2)}</td>
                    <td className="small">
                      {r.c ? (
                        <>
                          {fmtUtc(nsToDate(r.c.timestamp_ns))}
                          {lead !== null && lead > 0 && <div className="muted">{fmtDuration(lead)} before open</div>}
                        </>
                      ) : (
                        <span className="muted">{r.check === "missing" ? "never" : "…"}</span>
                      )}
                    </td>
                    <td>{r.err ? <span className="chip bad">RPC error</span> : <CheckChip check={r.check} />}</td>
                    <td className="num-cell mono">
                      {r.e.ret !== null ? `${(r.e.ret * 100).toFixed(2)}%` : <span className="muted">{r.e.status}</span>}
                    </td>
                  </tr>
                );
              })}
              {sealed.map((c) => {
                const d = labelDate(c.label);
                const lead = d ? entryKnownAt(d, null).getTime() - nsToDate(c.timestamp_ns).getTime() : null;
                const open = lead !== null && lead > 0 && entryKnownAt(d!, null) > new Date();
                return (
                  <tr key={c.hash}>
                    <td className="mono">{d ?? "?"}</td>
                    <td>
                      <span className="muted">Sealed signal</span>
                      <div className="hash small">{short(c.hash, 12)}</div>
                    </td>
                    <td className="num-cell muted" colSpan={3}>
                      {open ? "for sale until the open" : "not yet published by the seller"}
                    </td>
                    <td className="small">
                      {fmtUtc(nsToDate(c.timestamp_ns))}
                      {lead !== null && lead > 0 && <div className="muted">{fmtDuration(lead)} before open</div>}
                    </td>
                    <td>
                      <span className="chip lock">sealed</span>
                    </td>
                    <td className="num-cell muted">—</td>
                  </tr>
                );
              })}
              {data && rows.length === 0 && sealed.length === 0 && (
                <tr>
                  <td colSpan={8} className="empty">
                    No signals yet.
                  </td>
                </tr>
              )}
              {!data && !error && (
                <tr>
                  <td colSpan={8} className="empty checking">
                    Reading the chain…
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        {data && !data.record && (
          <p className="muted small">No published record file found (track-record.json); showing on-chain commitments only.</p>
        )}
      </div>

      <MemePanel data={data} />

      <JobsPanel data={data} />

      <div className="grid-2">
        <ReputationPanel data={data} />
        <PoolsPanel data={data} />
      </div>

      {/* ---------------- services ---------------- */}
      <div className="panel">
        <div className="panel-head">
          <div>
            <h2 className="h3">For sale</h2>
            <p className="muted small">
              Services registered on the contract. Agents pay per call through their owner&apos;s wallet; see the{" "}
              <a href="../docs/#buying">buying guide</a>.
            </p>
          </div>
        </div>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Service</th>
                <th>Seller</th>
                <th className="num-cell">Price / call</th>
                <th className="num-cell">Sold</th>
                <th className="num-cell">Earned</th>
              </tr>
            </thead>
            <tbody>
              {(data?.services ?? []).map(([id, s]) => (
                <tr key={id}>
                  <td>
                    <b className="mono">{id}</b>
                    <div className="muted small">{s.description}</div>
                  </td>
                  <td className="mono small">
                    <a href={explorerAccount(s.provider)} target="_blank" rel="noreferrer">
                      {s.provider}
                    </a>
                  </td>
                  <td className="num-cell mono">{fmtNear(s.price)} Ⓝ</td>
                  <td className="num-cell mono">{s.calls}</td>
                  <td className="num-cell mono">{fmtNear(s.earned)} Ⓝ</td>
                </tr>
              ))}
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
      </div>

      {/* ---------------- receipts ---------------- */}
      <div className="panel">
        <div className="panel-head">
          <div>
            <h2 className="h3">Payments between agents</h2>
            <p className="muted small">
              Latest receipts. Each is bound to the SHA-256 of the exact request; the seller serves only that request, once.
            </p>
          </div>
          {data && <span className="meta-row">{data.receiptCount} total</span>}
        </div>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>#</th>
                <th>Time</th>
                <th>Service</th>
                <th>Buyer agent → seller</th>
                <th>Request</th>
                <th className="num-cell">Paid</th>
              </tr>
            </thead>
            <tbody>
              {(data?.receipts ?? []).map((r) => (
                <tr key={r.id}>
                  <td className="mono">{r.id}</td>
                  <td className="small">{fmtUtc(nsToDate(r.timestamp_ns))}</td>
                  <td className="mono small">{r.service_id}</td>
                  <td className="mono small">
                    {r.payer} → {r.provider}
                  </td>
                  <td className="small">
                    {r.memo}
                    <div className="hash small">{short(r.request_hash, 10)}</div>
                  </td>
                  <td className="num-cell mono">{fmtNear(r.amount)} Ⓝ</td>
                </tr>
              ))}
              {data && data.receipts.length === 0 && (
                <tr>
                  <td colSpan={6} className="empty">
                    No payments yet.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      <VerifyYourself defaultCommitter={seller} />
    </div>
  );
}

function VerifyYourself({ defaultCommitter }: { defaultCommitter: string }) {
  const [line, setLine] = useState("axiom-signal/v1|JPM|trend-pullback|2026-11-09|339.6900|323.9929|371.0841|20");
  const [committer, setCommitter] = useState(defaultCommitter);
  const [out, setOut] = useState<{ hash: string; c: Commitment | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setBusy(true);
    setErr(null);
    try {
      const hash = await sha256Hex(line.trim());
      const c = await view<Commitment | null>("find_commitment", { committer: committer.trim(), hash });
      setOut({ hash, c });
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="panel" style={{ paddingBottom: 72 }}>
      <div className="panel-head">
        <div>
          <h2 className="h3">Verify it yourself</h2>
          <p className="muted small">
            Paste any signal line (format in the <a href="../docs/#verification">docs</a>). Your browser hashes it and asks the contract
            whether that seller committed it, and when. Change one digit and watch it fail.
          </p>
        </div>
      </div>
      <div className="verify-box">
        <textarea rows={2} value={line} onChange={(e) => setLine(e.target.value)} aria-label="Signal line" spellCheck={false} />
        <input value={committer} onChange={(e) => setCommitter(e.target.value)} aria-label="Committer account" spellCheck={false} />
        <div>
          <button className="btn sm" onClick={run} disabled={busy}>
            {busy ? "Checking…" : "Hash and look up"}
          </button>
        </div>
        {err && <div className="notice">{err}</div>}
        {out && (
          <div className="result-line">
            <span className="hash">{out.hash}</span>
            {out.c ? (
              <span className="chip ok">
                ✓ committed {fmtUtc(nsToDate(out.c.timestamp_ns))} · block {out.c.block_height.toLocaleString()}
              </span>
            ) : (
              <span className="chip bad">not committed by {committer}</span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
