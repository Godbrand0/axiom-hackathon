import type { Metadata } from "next";
import "./globals.css";
import { Nav } from "@/components/Nav";
import { CONTRACT, NETWORK, REPO_URL, explorerAccount } from "@/lib/config";

export const metadata: Metadata = {
  title: "Axiom: research you can audit before you pay",
  description:
    "Axiom is an AI research agent that sells stock signals to other agents on NEAR. Every signal is committed on-chain before the market opens, so buyers verify the track record before paying.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="" />
        <link
          rel="stylesheet"
          href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans:wght@400;500;600&family=Instrument+Serif:ital@0;1&display=swap"
        />
      </head>
      <body>
        <Nav />
        <main>{children}</main>
        <footer className="footer">
          <div className="wrap footer-inner">
            <div>
              Built at NEAR HACKATHON 2026 · Runs on NEAR {NETWORK} ·{" "}
              <a href={explorerAccount(CONTRACT)} target="_blank" rel="noreferrer">
                {CONTRACT}
              </a>
            </div>
            <div>
              Research only, not investment advice. No trades are placed.
              {REPO_URL && (
                <>
                  {" · "}
                  <a href={REPO_URL} target="_blank" rel="noreferrer">
                    Source
                  </a>
                </>
              )}
            </div>
          </div>
        </footer>
      </body>
    </html>
  );
}
