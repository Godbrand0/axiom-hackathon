/** Memecoin signals: the same canonical line and hash the Rust agent commits on-chain. */
import { Commitment, nsToDate, sha256Hex, view } from "./near";

export const MEME_LABEL = "axiom-meme/v1";

export type MemeRisk = { score: number; level: string; flags: string[]; top10_share: number | null };

export type MemeSignal = {
  token: string;
  symbol: string;
  pool: string;
  setup: string;
  signal_time: string;
  entry: number;
  stop: number;
  target: number;
  max_hold_h: number;
  risk: MemeRisk;
  committed_at: string | null;
};

export type MemeOutcome = { status: string; fill: number | null; exit: number | null; ret: number | null; held_h: number };

export type MemeRecord = {
  seller: string;
  committer: string;
  contract: string;
  network: string;
  exported_at: string;
  entries: MemeSignal[];
  outcomes?: { hash: string; outcome: MemeOutcome | null }[];
  active_signals: number;
  unrevealed?: number;
};

/** Rust's `{:.6e}`: like JS toExponential(6) but with no "+" in the exponent ("1.5e3", "2.0e-5"). */
export function exp6(x: number): string {
  return x.toExponential(6).replace("e+", "e");
}

export function memeLine(s: MemeSignal): string {
  const t = new Date(s.signal_time).toISOString().slice(0, 16) + "Z";
  return `${MEME_LABEL}|${s.token}|${s.pool}|${s.setup}|${t}|${exp6(s.entry)}|${exp6(s.stop)}|${exp6(s.target)}|${s.max_hold_h}`;
}

export const isMemeCommitment = (c: Commitment) => c.label.startsWith(MEME_LABEL);

export type MemeCheck = "ontime" | "late" | "missing";

/**
 * A memecoin signal counts if its commitment landed on-chain before the hour it is scored from began
 * (scoring starts at the first full hour after the commitment, so this holds by construction; a seller
 * could still publish a different commitment time than the chain's, which this catches).
 */
export async function verifyMeme(s: MemeSignal, committer: string): Promise<{ hash: string; check: MemeCheck; c: Commitment | null }> {
  const hash = await sha256Hex(memeLine(s));
  const c = await view<Commitment | null>("find_commitment", { committer, hash });
  if (!c) return { hash, check: "missing", c: null };
  const chainAt = nsToDate(c.timestamp_ns).getTime();
  const claimed = s.committed_at ? new Date(s.committed_at).getTime() : null;
  const ok = claimed === null || Math.abs(chainAt - claimed) < 120_000;
  return { hash, check: ok ? "ontime" : "late", c };
}

/** Compact price for sub-cent tokens: 0.000012345 -> "1.235e-5". */
export function fmtPrice(x: number): string {
  if (x >= 0.01) return x.toPrecision(4);
  return x.toExponential(3).replace("e+", "e");
}
