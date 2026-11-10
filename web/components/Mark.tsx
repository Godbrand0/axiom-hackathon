/** Axiom mark: a ledger rule crossed by a check, i.e. a verified entry. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" aria-hidden>
      <rect x="1.5" y="1.5" width="21" height="21" rx="6" stroke="currentColor" strokeWidth="1.6" />
      <path d="M6 15.5h12" stroke="currentColor" strokeWidth="1.2" strokeDasharray="1.6 1.6" opacity=".55" />
      <path d="M6.5 11.5l3.4 3.4L17.5 7.5" stroke="var(--accent)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
