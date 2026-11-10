# Axiom website

Next.js static site: landing page (`/`), live market (`/market`) and docs (`/docs`). It has no backend: the browser reads NEAR
testnet directly through FastNEAR's public RPC and re-hashes Axiom's published track record itself.

```bash
npm install
npm run dev            # http://localhost:3000
npm run build          # static export to out/
npm run preview        # serve out/ on http://127.0.0.1:4310
```

The track record comes from `public/track-record.json`. Refresh it after each scan, once signals' entries are known:

```bash
axiom near export-record --as <seller account>   # from the repo root; writes web/public/track-record.json
```

Build-time settings (all optional): `NEXT_PUBLIC_AXIOM_PAY_CONTRACT`, `NEXT_PUBLIC_AXIOM_SELLER`, `NEXT_PUBLIC_NEAR_NETWORK`,
`NEXT_PUBLIC_NEAR_RPC_URL`, `NEXT_PUBLIC_REPO_URL`, `NEXT_PUBLIC_BASE_PATH` (set automatically by the GitHub Pages workflow in
`.github/workflows/pages.yml`).
