import { useId } from 'react';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Pill } from '../ui/Pill';
import { IconAgentPresetOutline16, IconDataOutline16, IconFolderOpenOutline16 } from '../ui/icons';
import type { ProfileVisibility } from './client';
import './ProfileCard.css';

export function ProfileCard({
  name,
  description,
  route,
  onNavigate,
  visibility,
  revision,
  retired = false,
  author,
  usesCount,
  timestamp,
  timestampLabel,
}: {
  readonly name: string;
  readonly description: string;
  readonly route: Extract<AppRoute, { kind: 'profile' | 'marketplace_profile' }>;
  readonly onNavigate: (route: AppRoute) => void;
  readonly visibility: ProfileVisibility;
  readonly revision?: number;
  readonly retired?: boolean;
  readonly author?: string;
  readonly usesCount: number;
  readonly timestamp: string;
  readonly timestampLabel: 'Updated' | 'Published';
}) {
  const heading = useId();
  const kind = route.profileKind;
  return (
    <article className="profile-card" aria-labelledby={heading}>
      <div className="profile-card__kind">
        {kind === 'sandbox' ? (
          <IconDataOutline16 />
        ) : kind === 'workspace' ? (
          <IconFolderOpenOutline16 />
        ) : (
          <IconAgentPresetOutline16 />
        )}
        <span>{kind === 'sandbox' ? 'Sandbox' : kind === 'workspace' ? 'Workspace' : 'Agent'}</span>
      </div>
      <h2 id={heading}>
        <a
          href={routePath(route)}
          onClick={(event) => {
            if (
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            )
              return;
            event.preventDefault();
            onNavigate(route);
          }}
        >
          {name}
        </a>
      </h2>
      <div className="profile-card__badges">
        <Pill>{visibility === 'public' ? 'Public' : 'Private'}</Pill>
        <span>{revision === undefined ? 'Unpublished' : `Revision ${String(revision)}`}</span>
        {retired ? <Pill>Retired</Pill> : null}
      </div>
      <p className="profile-card__description">{description}</p>
      <footer className="profile-card__footer">
        {author === undefined ? null : <span className="profile-card__author">By {author}</span>}
        <span>
          {usesCount} {usesCount === 1 ? 'use' : 'uses'}
        </span>
        <time
          dateTime={timestamp}
          title={`${timestampLabel} ${new Date(timestamp).toLocaleString()}`}
        >
          {timestampLabel} {new Date(timestamp).toLocaleDateString()}
        </time>
      </footer>
    </article>
  );
}
