/** The Unsub mark: a record seen from above, inked like iTunes' idle LCD logo. */
export function VinylMark({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" role="img" aria-label="Unsub">
      <defs>
        <linearGradient id="vinyl-ink" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--lcd-mark-a)" />
          <stop offset="1" stopColor="var(--lcd-mark-b)" />
        </linearGradient>
        <linearGradient id="vinyl-sheen" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0.3" stopColor="#fff" stopOpacity="0" />
          <stop offset="0.5" stopColor="#fff" stopOpacity="0.18" />
          <stop offset="0.7" stopColor="#fff" stopOpacity="0" />
        </linearGradient>
      </defs>
      <circle cx="16" cy="16" r="15.5" fill="url(#vinyl-ink)" />
      <g fill="none" stroke="#6b6d5f" strokeWidth="0.45" opacity="0.8">
        <circle cx="16" cy="16" r="13" />
        <circle cx="16" cy="16" r="11.2" />
        <circle cx="16" cy="16" r="9.4" />
      </g>
      <circle cx="16" cy="16" r="15.5" fill="url(#vinyl-sheen)" />
      <circle cx="16" cy="16" r="6" fill="var(--lcd-top-b)" />
      <circle cx="16" cy="16" r="1.4" fill="url(#vinyl-ink)" />
    </svg>
  );
}
