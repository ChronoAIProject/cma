import { Presence } from '../../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import { CommandKeys } from '../../api/client';
import type { AppRoute, ProfileKind } from '../../routing/routes';
import { Button } from '../../ui/Button';
import { IconCopyOutline16, IconPlayOutline16 } from '../../ui/icons';
import { profileFailure, type ProfileFailure } from '../failure';
import type {
  MarketplaceClient,
  MarketplaceEntry,
  ProfileUse,
  ResolvedProfileRevision,
} from './client';
import { RevisionContents } from './RevisionContents';
import '../Profiles.css';
import './Marketplace.css';

export interface ProfileDetailProps {
  readonly client: Pick<MarketplaceClient, 'readMarketplace' | 'forkProfile'>;
  readonly kind: ProfileKind;
  readonly profileId: string;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onUse?: (selection: ProfileUse) => void;
  readonly canManage?: boolean;
}

function topRevision(entry: MarketplaceEntry): ResolvedProfileRevision {
  switch (entry.kind) {
    case 'agent':
      return { kind: 'agent', name: entry.summary.name, revision: entry.revision };
    case 'workspace':
      return { kind: 'workspace', name: entry.summary.name, revision: entry.revision };
    case 'sandbox':
      return { kind: 'sandbox', name: entry.summary.name, revision: entry.revision };
  }
}

export function ProfileDetail(props: ProfileDetailProps) {
  return <Detail key={`${props.kind}:${props.profileId}`} {...props} />;
}

function Detail({
  client,
  kind,
  profileId,
  onNavigate,
  onUse,
  canManage = true,
}: ProfileDetailProps) {
  const [entry, setEntry] = useState<MarketplaceEntry>();
  const [readError, setReadError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<ProfileFailure>();
  const [forkedId, setForkedId] = useState<string>();
  const keys = useRef(new CommandKeys());
  const write = useRef<AbortController | undefined>(undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const navigate = useRef(onNavigate);
  useEffect(() => {
    navigate.current = onNavigate;
  }, [onNavigate]);
  useEffect(() => {
    if (forkedId !== undefined)
      navigate.current({ kind: 'profile', profileKind: kind, profileId: forkedId });
  }, [forkedId, kind]);
  useEffect(() => () => write.current?.abort(), []);
  useEffect(() => {
    const request = new AbortController();
    void client
      .readMarketplace(kind, profileId, { signal: request.signal })
      .then((record) => {
        if (request.signal.aborted) return;
        if (
          record.kind !== kind ||
          record.summary.id !== profileId ||
          record.revision.profileId !== profileId
        )
          throw new Error('Profile identity mismatch');
        setEntry(record);
        setReadError(undefined);
      })
      .catch(() => {
        if (!request.signal.aborted) setReadError('This marketplace profile could not be loaded.');
      });
    return () => request.abort();
  }, [client, kind, profileId, reload]);
  useEffect(() => {
    document.title = `${entry?.summary.name ?? 'Profile'} · Marketplace · CMA`;
    if (entry !== undefined) heading.current?.focus();
    return () => {
      document.title = 'CMA';
    };
  }, [entry]);
  const fork = async () => {
    if (entry === undefined || write.current !== undefined || !canManage) return;
    const request = new AbortController();
    write.current = request;
    setPending(true);
    setFailure(undefined);
    try {
      const profile = await client.forkProfile(
        kind,
        profileId,
        { revision: entry.revision.revision },
        { idempotencyKey: keys.current.for('fork'), signal: request.signal },
      );
      if (request.signal.aborted) return;
      keys.current.settle('fork');
      setForkedId(profile.id);
    } catch (error: unknown) {
      if (request.signal.aborted) return;
      const problem = profileFailure(error);
      if (!problem.uncertain) keys.current.settle('fork');
      setFailure(problem);
    } finally {
      write.current = undefined;
      if (!request.signal.aborted) setPending(false);
    }
  };
  if (readError !== undefined)
    return (
      <section className="marketplace" role="alert">
        <h1>Profile unavailable</h1>
        <p>{readError}</p>
        <Button onClick={() => setReload((version) => version + 1)}>Try again</Button>
      </section>
    );
  if (entry === undefined)
    return (
      <section className="marketplace" aria-busy="true">
        <h1>Marketplace profile</h1>
        <p role="status">Loading profile</p>
      </section>
    );
  return (
    <section className="marketplace marketplace-detail">
      <header className="marketplace-detail-header">
        <p className="profile-editor__eyebrow">
          {kind === 'sandbox' ? 'Sandbox' : kind === 'workspace' ? 'Workspace' : 'Agent'} profile
        </p>
        <h1 ref={heading} tabIndex={-1}>
          {entry.summary.name}
        </h1>
        <p>{entry.summary.description}</p>
        <p>
          By {entry.summary.ownerDisplay} · Revision {entry.revision.revision} ·{' '}
          {entry.summary.usesCount} uses
        </p>
      </header>
      <section className="profile-editor__band">
        <h2>Published settings</h2>
        <RevisionContents value={topRevision(entry)} names={entry.resolvedProfiles} />
      </section>
      {entry.resolvedRevisions.map((value) => (
        <section
          className="profile-editor__band"
          key={`${value.kind}:${value.revision.profileId}:${String(value.revision.revision)}`}
        >
          <h2>
            {value.name} · Revision {value.revision.revision}
          </h2>
          <RevisionContents value={value} names={entry.resolvedProfiles} />
        </section>
      ))}
      {entry.kind === 'sandbox' && entry.trust.requiresConfirmation ? (
        <p className="profile-editor__notice">
          This profile includes commands or environment values from another author. Review them
          before confirming sandbox creation.
        </p>
      ) : null}
      <Presence>
        {failure === undefined ? null : (
          <div role="alert">
            <p>{failure.message}</p>
            {failure.uncertain ? (
              <Button disabled={pending} onClick={() => void fork()}>
                Retry private copy
              </Button>
            ) : null}
          </div>
        )}
      </Presence>
      <footer className="profile-editor__actions">
        {onUse === undefined ? null : (
          <Button
            variant="primary"
            icon={<IconPlayOutline16 />}
            disabled={pending || failure?.uncertain}
            onClick={() =>
              onUse({ kind, profile: { id: profileId, revision: entry.revision.revision }, entry })
            }
          >
            Use
          </Button>
        )}
        {canManage ? (
          <Button
            icon={<IconCopyOutline16 />}
            disabled={pending || failure?.uncertain || forkedId !== undefined}
            onClick={() => void fork()}
          >
            {pending ? 'Creating private copy' : 'Customize a private copy'}
          </Button>
        ) : null}
      </footer>
      {canManage ? (
        <p className="profile-field__help">
          Use the shared profile as published, or create your own private copy to edit its settings.
          {kind === 'sandbox'
            ? ' A private copy starts a fresh sandbox and does not use the original profile’s warm sandboxes.'
            : ''}
        </p>
      ) : null}
      {onUse === undefined ? (
        <p className="profile-field__help">
          Profile based creation is retired. Open New agent in Agent Manager to configure an image
          and inline Agent setup.
        </p>
      ) : null}
    </section>
  );
}
