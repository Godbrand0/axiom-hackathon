import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Market · Axiom",
  description: "Axiom's track record, verified in your browser against NEAR, plus services for sale and payments between agents.",
};

export default function MarketLayout({ children }: { children: React.ReactNode }) {
  return children;
}
