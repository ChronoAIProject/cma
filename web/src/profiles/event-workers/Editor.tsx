import { useEffect, useRef, useState } from 'react';
import { readIntent, retainIntent, clearIntent, type WorkerSaveIntent } from './intent';
import type { EventWorkerProfileRecord, EventWorkerProfiles } from '@chronoai/cma-sdk';
import type { CreationClient, CreationProfile } from '../../create/client';
import { ProfilePicker } from '../../create/ProfilePicker';
import type { ProfileKind, AppRoute } from '../../routing/routes';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { problem, definiteRefusal } from '../../bots/model';
import '../Profiles.css';
import '../../create/Create.css';

type Client = Pick<EventWorkerProfiles, 'get' | 'create' | 'update' | 'retire'>;
const kinds = ['sandbox', 'workspace', 'agent'] as const;
export function EventWorkerEditor({
  client,
  profiles,
  profileId,
  viewerId,
  onNavigate,
}: {
  readonly client: Client;
  readonly profiles: CreationClient;
  readonly profileId: string;
  readonly viewerId: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [recovery] = useState(() => {
    try {
      return { intent: readIntent(viewerId, profileId), error: undefined };
    } catch {
      return {
        intent: undefined,
        error: 'The retained save could not be read. Restore browser storage access before saving.',
      };
    }
  });
  const [record, setRecord] = useState<EventWorkerProfileRecord>();
  const [name, setName] = useState(recovery.intent?.body.name ?? '');
  const [description, setDescription] = useState(recovery.intent?.body.description ?? '');
  const [selected, setSelected] = useState<Partial<Record<ProfileKind, CreationProfile>>>({});
  const [loading, setLoading] = useState(profileId !== 'new');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | undefined>(recovery.error);
  const [confirmRetire, setConfirmRetire] = useState(false);
  const [reload, setReload] = useState(0);
  const pending = useRef<WorkerSaveIntent | undefined>(recovery.intent);
  const [uncertain, setUncertain] = useState(recovery.intent !== undefined);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    document.title = 'Event Worker Profile · CMA';
    return () => {
      document.title = 'CMA';
    };
  }, []);
  useEffect(() => {
    if (profileId === 'new') return;
    const abort = new AbortController();
    void client
      .get(profileId, { signal: abort.signal })
      .then(async (value) => {
        if (abort.signal.aborted) return;
        setRecord(value);
        setName(pending.current?.body.name ?? value.source.name);
        setDescription(pending.current?.body.description ?? value.source.description);
        const choices = await Promise.allSettled(
          kinds.map(async (kind) => {
            const reference = (pending.current?.body ?? value.source)[`${kind}_profile`];
            const revision = await profiles.readProfile(
              kind,
              { ...reference, name: kind, source: 'own' },
              { signal: abort.signal },
            );
            return [kind, revision] as const;
          }),
        );
        if (abort.signal.aborted) return;
        setSelected(
          Object.fromEntries(
            choices.flatMap((choice) => (choice.status === 'fulfilled' ? [choice.value] : [])),
          ),
        );
        setLoading(false);
        setError(
          choices.some((choice) => choice.status === 'rejected')
            ? 'Some source profiles are unavailable. The saved worker configuration is retained; select available sources to save a new revision.'
            : undefined,
        );
      })
      .catch((failure: unknown) => {
        if (!abort.signal.aborted) {
          setLoading(false);
          setError(problem(failure));
        }
      });
    return () => abort.abort();
  }, [client, profiles, profileId, reload]);
  const save = async () => {
    if (busy || recovery.error) return;
    const { agent, workspace, sandbox } = selected;
    if (pending.current === undefined) {
      if (!agent || !workspace || !sandbox || name.trim() === '') return;
      const reference = (value: CreationProfile) => ({
        id: value.revision.profileId,
        revision: value.revision.revision,
      });
      pending.current = {
        key: crypto.randomUUID(),
        expectedRevision: record?.revision,
        body: {
          name,
          description,
          agent_profile: reference(agent),
          workspace_profile: reference(workspace),
          sandbox_profile: reference(sandbox),
        },
      };
    }
    const intent = pending.current;
    setBusy(true);
    setError(undefined);
    try {
      retainIntent(viewerId, profileId, intent);
      const value =
        intent.expectedRevision === undefined
          ? await client.create(intent.body, { idempotencyKey: intent.key })
          : await client.update(profileId, intent.expectedRevision, intent.body);
      clearIntent(viewerId, profileId);
      pending.current = undefined;
      if (!mounted.current) return;
      setRecord(value);
      setUncertain(false);
      onNavigate({ kind: 'event_worker_profile', profileId: value.id });
    } catch (failure) {
      if (definiteRefusal(failure)) {
        clearIntent(viewerId, profileId);
        pending.current = undefined;
        if (!mounted.current) return;
        setUncertain(false);
      } else {
        if (!mounted.current) return;
        setUncertain(true);
      }
      setError(problem(failure));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  const retire = async () => {
    if (!record || busy) return;
    setBusy(true);
    setError(undefined);
    try {
      await client.retire(record.id, record.revision);
      if (mounted.current) onNavigate({ kind: 'event_worker_profiles' });
    } catch (failure) {
      if (mounted.current) setError(problem(failure));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  const locked =
    busy || loading || uncertain || recovery.error !== undefined || record?.retired_at != null;
  return (
    <section className="profile-editor">
      <header className="profile-editor__header">
        <h1>{profileId === 'new' ? 'New Event Worker Profile' : name || 'Event Worker Profile'}</h1>
        <Button onClick={() => onNavigate({ kind: 'event_worker_profiles' })}>
          Back to profiles
        </Button>
      </header>
      <p>
        Choose published CMA profiles. Each save pins their complete configuration; existing Bots
        keep their selected revision.
      </p>
      {uncertain ? (
        <p role="status">
          A previous save needs confirmation. Retry the same save to recover its result.
        </p>
      ) : null}
      {loading ? <p role="status">Loading profile</p> : null}
      {error === undefined ? null : (
        <div role="alert">
          <p>{error}</p>
          {profileId !== 'new' && record === undefined ? (
            <Button
              onClick={() => {
                setLoading(true);
                setReload((n) => n + 1);
              }}
            >
              Try again
            </Button>
          ) : null}
        </div>
      )}
      {record?.retired_at != null ? (
        <p>This profile is retired. Existing Bots retain their configuration.</p>
      ) : null}
      {record === undefined && profileId !== 'new' ? null : (
        <>
          <section className="profile-editor__band">
            <div className="profile-field">
              <label htmlFor="worker-name">Name</label>
              <Input
                id="worker-name"
                value={name}
                disabled={locked}
                maxLength={80}
                onChange={(event) => setName(event.target.value)}
              />
            </div>
            <div className="profile-field">
              <label htmlFor="worker-description">Description</label>
              <textarea
                id="worker-description"
                className="profile-textarea"
                rows={3}
                value={description}
                disabled={locked}
                maxLength={2000}
                onChange={(event) => setDescription(event.target.value)}
              />
            </div>
          </section>
          {kinds.map((kind) => (
            <section className="profile-editor__band" key={kind}>
              <h2>
                {kind === 'sandbox'
                  ? 'Sandbox environment'
                  : kind === 'workspace'
                    ? 'Workspace'
                    : 'Agent configuration'}
              </h2>
              <ProfilePicker
                client={profiles}
                kind={kind}
                personalOnly
                value={selected[kind]}
                disabled={locked}
                onChange={(value) => setSelected((current) => ({ ...current, [kind]: value }))}
              />
            </section>
          ))}
          {record === undefined ? null : (
            <p>
              Saved revision {record.revision} · {record.profile.model.model}
            </p>
          )}
          {record?.retired_at == null ? (
            <Button
              variant="primary"
              disabled={
                busy ||
                loading ||
                recovery.error !== undefined ||
                (!uncertain &&
                  (!selected.agent ||
                    !selected.workspace ||
                    !selected.sandbox ||
                    name.trim() === ''))
              }
              onClick={() => void save()}
            >
              {busy
                ? 'Saving…'
                : uncertain
                  ? 'Retry the same save'
                  : record
                    ? 'Save new revision'
                    : 'Create worker profile'}
            </Button>
          ) : null}
          {record !== undefined && record.retired_at === null ? (
            <section className="profile-editor__band">
              <h2>Retire profile</h2>
              <p>
                Remove this profile from new Bot choices. Saved revisions and existing Bots remain
                available.
              </p>
              {confirmRetire ? (
                <>
                  <Button disabled={busy || uncertain} onClick={() => void retire()}>
                    Confirm retirement
                  </Button>
                  <Button disabled={busy} onClick={() => setConfirmRetire(false)}>
                    Cancel
                  </Button>
                </>
              ) : (
                <Button disabled={busy || uncertain} onClick={() => setConfirmRetire(true)}>
                  Retire profile
                </Button>
              )}
            </section>
          ) : null}
        </>
      )}
    </section>
  );
}
