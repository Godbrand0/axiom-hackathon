"use client";
import { useCallback, useEffect, useState } from "react";
import { BASE_PATH, SELLER } from "./config";
import { MemeRecord } from "./meme";
import { Commitment, Job, Pool, Receipt, SellerStats, Service, TrackRecord, view } from "./near";

export type Market = {
  services: [string, Service][];
  receiptCount: number;
  commitmentCount: number;
  receipts: Receipt[];
  /** The seller's commitments, newest first. */
  sellerCommitments: Commitment[];
  record: TrackRecord | null;
  memeRecord: MemeRecord | null;
  /** Newest first. */
  jobs: Job[];
  jobCount: number;
  pools: Pool[];
  poolCount: number;
  /** On-chain delivery record of every seller that has won or bid on a job shown. */
  sellers: [string, SellerStats][];
  loadedAt: Date;
};

async function loadJson<T>(file: string): Promise<T | null> {
  try {
    const res = await fetch(`${BASE_PATH}/${file}`, { cache: "no-store" });
    return res.ok ? ((await res.json()) as T) : null;
  } catch {
    return null;
  }
}

export async function loadMarket(): Promise<Market> {
  const [services, receiptCount, commitmentCount, jobCount, poolCount, record, memeRecord] = await Promise.all([
    view<[string, Service][]>("list_services"),
    view<number>("receipt_count"),
    view<number>("commitment_count"),
    view<number>("job_count"),
    view<number>("pool_count"),
    loadJson<TrackRecord>("track-record.json"),
    loadJson<MemeRecord>("meme-record.json"),
  ]);
  const last = 25;
  const [receipts, commitments, jobs, pools] = await Promise.all([
    view<Receipt[]>("get_receipts", { from: Math.max(0, receiptCount - last), limit: last }),
    view<Commitment[]>("get_commitments", { from: 0, limit: commitmentCount }),
    view<Job[]>("get_jobs", { from: Math.max(0, jobCount - last), limit: last }),
    view<Pool[]>("get_pools", { from: Math.max(0, poolCount - 10), limit: 10 }),
  ]);
  const sellerIds = [...new Set(jobs.map((j) => j.winner).filter((w): w is string => !!w))];
  const stats = await Promise.all(sellerIds.map((s) => view<SellerStats>("get_seller_stats", { seller: s })));
  const seller = record?.committer || SELLER;
  return {
    services,
    receiptCount,
    commitmentCount,
    receipts: [...receipts].reverse(),
    sellerCommitments: commitments.filter((c) => c.committer === seller).reverse(),
    record,
    memeRecord,
    jobs: [...jobs].reverse(),
    jobCount,
    pools: [...pools].reverse(),
    poolCount,
    sellers: sellerIds.map((s, i) => [s, stats[i]] as [string, SellerStats]).sort((a, b) => b[1].delivered - a[1].delivered),
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
