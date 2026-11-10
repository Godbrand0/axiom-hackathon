"use client";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { NETWORK, REPO_URL } from "@/lib/config";
import { Mark } from "./Mark";

const links = [
  { href: "/market/", label: "Market" },
  { href: "/docs/", label: "Docs" },
];

export function Nav() {
  const path = usePathname();
  return (
    <header className="nav">
      <div className="wrap nav-inner">
        <Link href="/" className="brand" aria-label="Axiom home">
          <Mark className="brand-mark" />
          Axiom
        </Link>
        <nav className="nav-links" aria-label="Main">
          {links.map((l) => (
            <Link key={l.href} href={l.href} aria-current={path?.startsWith(l.href.slice(0, -1)) ? "page" : undefined}>
              {l.label}
            </Link>
          ))}
          {REPO_URL && (
            <a href={REPO_URL} target="_blank" rel="noreferrer">
              GitHub
            </a>
          )}
        </nav>
        <span className="net-pill" title="Reads the NEAR blockchain live">
          <span className="dot" aria-hidden />
          {NETWORK}
        </span>
      </div>
    </header>
  );
}
