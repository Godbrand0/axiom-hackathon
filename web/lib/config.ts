/** Deployment the site reads. Override at build time with NEXT_PUBLIC_* env vars. */
export const NETWORK = process.env.NEXT_PUBLIC_NEAR_NETWORK || "testnet";
export const RPC_URL =
  process.env.NEXT_PUBLIC_NEAR_RPC_URL || (NETWORK === "mainnet" ? "https://free.rpc.fastnear.com" : "https://test.rpc.fastnear.com");
export const CONTRACT = process.env.NEXT_PUBLIC_AXIOM_PAY_CONTRACT || "axiom-market-33945.testnet";
export const SELLER = process.env.NEXT_PUBLIC_AXIOM_SELLER || "axiom-agent-33945.testnet";
export const REPO_URL = process.env.NEXT_PUBLIC_REPO_URL || "";
export const BASE_PATH = process.env.NEXT_PUBLIC_BASE_PATH || "";

export const EXPLORER = NETWORK === "mainnet" ? "https://nearblocks.io" : "https://testnet.nearblocks.io";
export const explorerAccount = (a: string) => `${EXPLORER}/address/${a}`;
export const explorerTx = (h: string) => `${EXPLORER}/txns/${h}`;
