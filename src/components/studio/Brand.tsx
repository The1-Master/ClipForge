/** Original ClipForge mark: angular C, negative-space play cut and forge spark. */
export function Brand({ compact = false, showTagline = true }: { compact?: boolean; showTagline?: boolean }) {
  return <span className={`cf-brand ${compact ? 'cf-brand-compact' : ''}`} aria-label="ClipForge">
    <svg className="cf-brand-mark" width="42" height="42" viewBox="0 0 64 64" aria-hidden="true" focusable="false">
      <path fill="currentColor" fillRule="evenodd" d="M18 10h25l9 9-11 8-5-5H23l-6 10 6 10h13l5-5 11 8-9 9H18L4 32 18 10Zm9 15v14l12-7-12-7Z" />
      <path className="cf-brand-spark" d="m53 2 2.5 7.5L63 12l-7.5 2.5L53 22l-2.5-7.5L43 12l7.5-2.5L53 2Z" fill="currentColor" />
    </svg>
    {!compact && <span className="cf-brand-copy"><strong>Clip<span>Forge</span></strong>{showTagline && <small>Your game. Your moments.</small>}</span>}
  </span>;
}
