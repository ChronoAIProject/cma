import type { ReactNode } from 'react';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import './Profiles.css';

const sections = [
  { kind: 'profiles', label: 'My profiles' },
  { kind: 'marketplace', label: 'Marketplace' },
  { kind: 'event_worker_profiles', label: 'Event workers' },
] as const;
export function ProfileLibrary({
  active,
  onNavigate,
  children,
}: {
  readonly active: (typeof sections)[number]['kind'];
  readonly onNavigate: (route: AppRoute) => void;
  readonly children: ReactNode;
}) {
  return (
    <div className="profile-library">
      <nav className="profile-tabs" aria-label="Profile library">
        {sections.map(({ kind, label }) => (
          <a
            key={kind}
            className="profile-library__link"
            href={routePath({ kind })}
            aria-current={kind === active ? 'page' : undefined}
            onClick={(event) => {
              if (!event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
                event.preventDefault();
                onNavigate({ kind });
              }
            }}
          >
            {label}
          </a>
        ))}
      </nav>
      {children}
    </div>
  );
}
