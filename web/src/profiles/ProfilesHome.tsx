import { useEffect, useRef, useState } from 'react';
import type { AppRoute, ProfileKind } from '../routing/routes';
import { Button } from '../ui/Button';
import { IconPlusOutline16 } from '../ui/icons';
import type { ProfilesClient, ProfileSummary } from './client';
import { PROFILE_KINDS, ProfileTabs } from './ProfileTabs';
import { ProfileCard } from './ProfileCard';
import './Profiles.css';

interface ListState {
  readonly items: readonly ProfileSummary[];
  readonly next?: string;
  readonly loading: boolean;
  readonly error?: string;
}

export function ProfilesHome({
  client,
  onNavigate,
  onNewProfile,
  initialKind = 'sandbox',
  canManage = true,
}: {
  readonly client: ProfilesClient;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onNewProfile: (kind: ProfileKind) => void;
  readonly initialKind?: ProfileKind;
  readonly canManage?: boolean;
}) {
  const [kind, setKind] = useState(initialKind);
  const [cursor, setCursor] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<ListState>({ items: [], loading: true });
  const heading = useRef<HTMLHeadingElement>(null);
  const request = useRef<AbortController | undefined>(undefined);
  const cursors = useRef(new Set<string>());
  useEffect(() => {
    document.title = 'Profiles · CMA';
    heading.current?.focus();
    return () => {
      document.title = 'CMA';
    };
  }, []);
  useEffect(() => {
    const abort = new AbortController();
    request.current = abort;
    void client
      .listProfiles(kind, { cursor, signal: abort.signal })
      .then((page) => {
        if (abort.signal.aborted) return;
        if (page.next !== undefined && (page.next === cursor || cursors.current.has(page.next)))
          throw new Error('Nonadvancing profile page');
        if (page.next !== undefined) cursors.current.add(page.next);
        setState((previous) => ({
          items:
            cursor === undefined
              ? page.items
              : [
                  ...previous.items,
                  ...page.items.filter(
                    (item) => !previous.items.some((current) => current.id === item.id),
                  ),
                ],
          next: page.next,
          loading: false,
        }));
      })
      .catch(() => {
        if (!abort.signal.aborted)
          setState((previous) => ({
            ...previous,
            loading: false,
            error: 'Profiles could not be loaded.',
          }));
      });
    return () => abort.abort();
  }, [client, kind, cursor, revision]);
  const label = PROFILE_KINDS.find((entry) => entry.kind === kind)?.label ?? 'Agent';
  return (
    <section className="profiles-home">
      <header className="profiles-home__header">
        <h1 ref={heading} tabIndex={-1}>
          Profiles
        </h1>
        {canManage ? (
          <Button icon={<IconPlusOutline16 />} variant="primary" onClick={() => onNewProfile(kind)}>
            New profile
          </Button>
        ) : null}
      </header>
      <ProfileTabs
        kind={kind}
        onChange={(next) => {
          if (next === kind) return;
          request.current?.abort();
          cursors.current.clear();
          setKind(next);
          setCursor(undefined);
          setState({ items: [], loading: true });
        }}
      />
      <div
        id="profile-list"
        role="tabpanel"
        aria-labelledby={`profile-tab-${kind}`}
        aria-busy={state.loading}
      >
        <ul className="profile-card-grid">
          {state.items.map((profile) => {
            const route: AppRoute = { kind: 'profile', profileKind: kind, profileId: profile.id };
            return (
              <li key={profile.id}>
                <ProfileCard
                  name={profile.name}
                  description={profile.description}
                  route={route}
                  onNavigate={onNavigate}
                  visibility={profile.visibility}
                  revision={profile.latestRevision?.revision}
                  retired={profile.retiredAt !== undefined}
                  usesCount={profile.usesCount}
                  timestamp={profile.updatedAt}
                  timestampLabel="Updated"
                />
              </li>
            );
          })}
        </ul>
        {state.loading ? <p role="status">Loading profiles</p> : null}
        {state.error === undefined ? (
          !state.loading && state.items.length === 0 ? (
            <p>No {label.toLowerCase()} profiles yet.</p>
          ) : null
        ) : (
          <div role="alert">
            <p>{state.error}</p>
            <Button
              onClick={() => {
                setState((previous) => ({ ...previous, loading: true, error: undefined }));
                setRevision((value) => value + 1);
              }}
            >
              Try again
            </Button>
          </div>
        )}
        {state.next === undefined ? null : (
          <Button
            disabled={state.loading}
            onClick={() => {
              setCursor(state.next);
              setState((previous) => ({ ...previous, loading: true }));
            }}
          >
            More profiles
          </Button>
        )}
      </div>
    </section>
  );
}
