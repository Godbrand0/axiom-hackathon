"use client";
import { useCallback, useEffect, useState } from "react";
import { BASE_PATH, SELLER } from "./config";
import { Commitment, Receipt, Service, TrackRecord, view } from "./near";

export type Market = {
  services: [string, Service][];
  receiptCount: number;
  commitmentCount: number;
  receipts: Receipt[];
  /** The seller's commitments, newest first. */
  sellerCommitments: Commitment[];
  record: TrackRecord | null;
  loadedAt: Date;
};

async function loadRecord(): Promise<TrackRecord | null> {
  try {
    const res = await fetch(`${BASE_PATH}/track-record.json`, { cache: "no-store" });
    return res.ok ? ((await res.json()) as TrackRecord) : null;
  } catch {
    return null;
  }
}

export async function loadMarket(): Promise<Market> {
  const [services, receiptCount, commitmentCount, record] = await Promise.all([
    view<[string, Service][]>("list_services"),
    view<number>("receipt_count"),
    view<number>("commitment_count"),
    loadRecord(),
  ]);
  const last = 25;
  const [receipts, commitments] = await Promise.all([
    view<Receipt[]>("get_receipts", { from: Math.max(0, receiptCount - last), limit: last }),
    view<Commitment[]>("get_commitments", { from: 0, limit: commitmentCount }),
  ]);
  const seller = record?.committer || SELLER;
  return {
    services,
    receiptCount,
    commitmentCount,
    receipts: [...receipts].reverse(),
    sellerCommitments: commitments.filter((c) => c.committer === seller).reverse(),
    record,
    loadedAt: new Date(),
  };
}

/** Components on one page share a single load (the public RPC rate-limits bursts). */
let shared: { at: number; p: Promise<Market> } | null = null;
function loadShared(force: boolean): Promise<Market> {
  if (force || !shared || Date.now() - shared.at > 15_000) {
    shared = { at: Date.now(), p: loadMarket() };
    shared.p.catch(() => (shared = null));
  }
  return shared.p;
}

export function useMarket() {
  const [data, setData] = useState<Market | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async (force = false) => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadShared(force));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh(false);
  }, [refresh]);

  return { data, error, loading, refresh };
}

/** "axiom-signal/v1 2026-11-09" or "JPM trend-pullback 2026-11-09" -> "2026-11-09". */
export function labelDate(label: string): string | null {
  const m = label.match(/(\d{4}-\d{2}-\d{2})\s*$/);
  return m ? m[1] : null;
}
