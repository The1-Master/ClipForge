import type { ReactNode } from 'react';
import { Icon } from './Icon.tsx';

export function EmptyState({ variant = 'clips', title, description, action }: {
  variant?: 'clips' | 'matches' | 'activity' | 'search' | 'error'; title: string; description?: string; action?: ReactNode;
}) {
  const icon = { clips: 'play', matches: 'matches', activity: 'check', search: 'search', error: 'info' }[variant];
  return <section className={`cf-empty cf-empty-${variant}`}>
    <div className="cf-empty-art" aria-hidden="true">
      <svg width="220" height="128" viewBox="0 0 240 140" focusable="false">
        <path className="cf-empty-orbit" d="M24 89C6 47 38 12 90 20c35 6 41-12 77-9 45 4 79 36 53 81-22 40-75 45-123 28-27-9-56-10-73-31Z" />
        <path className="cf-empty-card-back" d="m58 27 119-6 5 82-119 6Z" />
        <rect className="cf-empty-card-front" x="47" y="36" width="146" height="86" rx="12" />
        <path className="cf-empty-dashes" d="M58 50h14m96 0h14M58 108h14m96 0h14" />
        <path className="cf-empty-spark" d="m205 16 3 9 9 3-9 3-3 9-3-9-9-3 9-3 3-9ZM27 105l2 6 6 2-6 2-2 6-2-6-6-2 6-2 2-6Z" />
      </svg><Icon name={icon} size={34} />
    </div>
    <h2>{title}</h2>{description && <p>{description}</p>}{action && <div className="cf-empty-actions">{action}</div>}
  </section>;
}
