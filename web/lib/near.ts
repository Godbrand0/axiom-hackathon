/** Browser-side NEAR reads and the same hashing/verification rules the Rust agent uses. */
import { CONTRACT, RPC_URL } from "./config";

const YOCTO = 10n ** 24n;

export async function view<T>(method: string, args: Record<string, unknown> = {}, contract = CONTRACT): Promise<T> {
  const body = {
    jsonrpc: "2.0",
    id: "axiom-web",
    method: "query",
    params: {
      request_type: "call_function",
      finality: "final",
      account_id: contract,
      method_name: method,
      args_base64: btoa(JSON.stringify(args)),
    },
  };
  let lastErr: unknown;
  for (let attempt = 0; attempt < 4; attempt++) {
    if (attempt) await new Promise((r) => setTimeout(r, 700 * attempt));
    try {
      const res = await fetch(RPC_URL, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
      if (res.status === 429 || res.status >= 500) throw new Error(`RPC HTTP ${res.status}`);
      const json = await res.json();
      if (json.error) throw new Error(JSON.stringify(json.error));
      if (json.result?.error) throw new Error(json.result.error);
      const bytes = new Uint8Array(json.result.result as number[]);
      const text = new TextDecoder().decode(bytes);
      return (text ? JSON.parse(text) : null) as T;
    } catch (e) {
      lastErr = e;
    }
  }
  throw lastErr;
}

export async function sha256Hex(s: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** yoctoNEAR string -> "0.05". */
export function fmtNear(yocto: string | bigint, digits = 4): string {
  const y = typeof yocto === "bigint" ? yocto : BigInt(yocto || "0");
  const whole = y / YOCTO;
  const frac = (y % YOCTO).toString().padStart(24, "0").slice(0, digits).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : `${whole}`;
}

export const nsToDate = (ns: number | string) => new Date(Number(BigInt(ns) / 1_000_000n));

// ---- types mirroring the contract ----
export type Service = { provider: string; price: string; description: string; calls: number; earned: string };
export type Receipt = {
  id: number;
  service_id: string;
  provider: string;
  payer: string;
  owner: string;
  amount: string;
  request_hash: string;
  memo: string;
  timestamp_ns: number;
  block_height: number;
};
export type Commitment = { id: number; committer: string; hash: string; label: string; timestamp_ns: number; block_height: number };

export type RecordEntry = {
  symbol: string;
  setup: string;
  as_of: string;
  entry: number;
  stop: number;
  target: number;
  max_hold: number;
  status: string;
  fill_date: string | null;
  exit_date: string | null;
  ret: number | null;
};

export type TrackRecord = { seller: string; committer: string; contract: string; network: string; generated_at: string; entries: RecordEntry[] };

/**
 * Rust's `{:.4}`: rounds the exact binary value, ties to even. JS `toFixed` rounds ties up
 * (0.03125 -> "0.0313" vs Rust "0.0312"), which would break the hash, so ties are handled here.
 */
export function fixed4(x: number): string {
  const exact = Math.abs(x).toFixed(20); // exact for any double with <= 20 fractional binary digits that matter here
  const [int, frac] = exact.split(".");
  const tail = frac.slice(4);
  if (/^50*$/.test(tail)) {
    // Exact tie: keep the 4th digit if even, otherwise round up (same as toFixed).
    const kept = `${int}.${frac.slice(0, 4)}`;
    if (Number(frac[3]) % 2 === 0) return (x < 0 ? "-" : "") + kept;
  }
  return x.toFixed(4);
}

/** Same text the Rust agent hashes. */
export function signalLine(e: Pick<RecordEntry, "symbol" | "setup" | "as_of" | "entry" | "stop" | "target" | "max_hold">): string {
  return `axiom-signal/v1|${e.symbol}|${e.setup}|${e.as_of}|${fixed4(e.entry)}|${fixed4(e.stop)}|${fixed4(e.target)}|${e.max_hold}`;
}

/** 13:30 UTC on the fill day, or the next weekday after the signal: the earliest the entry could be known. */
export function entryKnownAt(asOf: string, fill: string | null): Date {
  if (fill) return new Date(`${fill}T13:30:00Z`);
  const d = new Date(`${asOf}T13:30:00Z`);
  d.setUTCDate(d.getUTCDate() + 1);
  while (d.getUTCDay() === 0 || d.getUTCDay() === 6) d.setUTCDate(d.getUTCDate() + 1);
  return d;
}

export type Check = "ontime" | "late" | "missing";

export async function verifyEntry(e: RecordEntry, committer: string): Promise<{ hash: string; check: Check; commitment: Commitment | null }> {
  const hash = await sha256Hex(signalLine(e));
  const c = await view<Commitment | null>("find_commitment", { committer, hash });
  if (!c) return { hash, check: "missing", commitment: null };
  const ok = nsToDate(c.timestamp_ns) <= entryKnownAt(e.as_of, e.fill_date);
  return { hash, check: ok ? "ontime" : "late", commitment: c };
}

export function fmtDuration(ms: number): string {
  const m = Math.round(Math.abs(ms) / 60000);
  const h = Math.floor(m / 60);
  return h ? `${h}h ${m % 60}m` : `${m}m`;
}

export const fmtUtc = (d: Date) => d.toISOString().replace("T", " ").slice(0, 16) + " UTC";

export type JobStatus = "Open" | "Awarded" | "Delivered" | "Disputed" | "Cancelled";
export type Job = {
  id: number;
  buyer: string;
  owner: string;
  topic: string;
  max_price: string;
  deadline_ns: number;
  status: JobStatus;
  winner: string | null;
  price: string;
  receipt: number | null;
  bids: number;
};
export type SellerStats = { awarded: number; delivered: number; disputed: number; earned: string };
export type PoolStatus = "Open" | "Filled" | "Refunded";
export type Pool = {
  id: number;
  service_id: string;
  provider: string;
  request_hash: string;
  memo: string;
  price: string;
  shares: number;
  members: string[];
  owners: string[];
  paid: string[];
  deadline_ns: number;
  status: PoolStatus;
  receipt: number | null;
};
