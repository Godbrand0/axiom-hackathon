"use client";
import { useEffect, useState } from "react";
import { explorerAccount } from "@/lib/config";
import { entryKnownAt, fmtDuration, fmtNear, fmtUtc, nsToDate, verifyEntry } from "@/lib/near";
import { labelDate, useMarket } from "@/lib/useMarket";

/** One real commitment from the chain, shown as a ledger slip. */
export function ProofSlip() {
  const { data, error } = useMarket();
  const [verified, setVerified] = useState<boolean | null>(null);

  const c = data?.sellerCommitments[0];
  const date = c ? labelDate(c.label) : null;
  const committedAt = c ? nsToDate(c.timestamp_ns) : null;
  const openAt = date ? entryKnownAt(date, null) : null;
  const lead = committedAt && openAt ? openAt.getTime() - committedAt.getTime() : null;
  const published = data?.record?.entries.find((e) => e.as_of === date);

  useEffect(() => {
    if (!published || !data?.record) return;
    verifyEntry(published, data.record.committer)
      .then((r) => setVerified(r.check === "ontime"))
      .catch(() => setVerified(null));
  }, [published, data?.record]);

  return (
    <div className="slip" aria-live="polite">
      <div className="slip-head">
        <span className="eyebrow">Latest commitment · live from NEAR</span>
        <span className="mono small muted">#{c ? c.id : "…"}</span>
      </div>
      <div className="slip-body">
        <div className="slip-row">
          <span>Seller</span>
          <span className="mono">
            {c ? (
              <a href={explorerAccount(c.committer)} target="_blank" rel="noreferrer">
                {c.committer}
              </a>
            ) : (
              "…"
            )}
          </span>
        </div>
        <div className="slip-row">
          <span>Session</span>
          <span>{date ?? "…"}</span>
        </div>
        <div className="slip-row">
          <span>SHA-256</span>
          <span className={`hash ${c ? "" : "checking"}`}>{c ? c.hash : "reading the chain…"}</span>
        </div>
        <div className="slip-row">
          <span>Committed</span>
          <span>
            {committedAt ? fmtUtc(committedAt) : "…"} {c && <span className="muted">· block {c.block_height.toLocaleString()}</span>}
          </span>
        </div>
        {error && <div className="notice">Could not reach NEAR right now: {error}</div>}
        {c && lead !== null && (
          <div className={`slip-verdict ${lead > 0 ? "" : "pending"}`}>
            <span className="big">{lead > 0 ? "✓" : "!"}</span>
            <span>
              {lead > 0 ? (
                <>
                  Recorded <b>{fmtDuration(lead)} before</b> the market opened on it.{" "}
                  {published
                    ? verified === true
                      ? "Contents published and re-hashed in your browser: match."
                      : verified === false
                        ? "Published contents do NOT match."
                        : "Checking the published contents…"
                    : "Contents stay sealed until the open; buyers can pay for them now."}
                </>
              ) : (
                <>Recorded after the open: this one cannot count toward the track record.</>
              )}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

export function LiveStats() {
  const { data } = useMarket();
  const earned = data?.services.reduce((sum, [, s]) => sum + BigInt(s.earned), 0n);
  const items = [
    { v: data?.commitmentCount, l: "signals committed on-chain" },
    { v: data?.receiptCount, l: "paid calls between agents" },
    { v: data?.services.length, l: "services for sale" },
    { v: earned !== undefined ? `${fmtNear(earned, 2)} Ⓝ` : undefined, l: "paid to sellers (testnet)" },
  ];
  return (
    <div className="stats">
      {items.map((it) => (
        <div className="stat" key={it.l}>
          <span className={`stat-value ${it.v === undefined ? "checking" : ""}`}>{it.v ?? "—"}</span>
          <span className="stat-label">{it.l}</span>
        </div>
      ))}
    </div>
  );
}
