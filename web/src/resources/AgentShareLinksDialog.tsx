import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ApiError,
  pendingRevocation,
  type AgentShareLink,
  type CreateAgentShareLinkRequest,
  type OrganizationChoice,
  type ShareIdentity,
} from '@cma/client';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { Picker } from '../ui/Picker';
import { AgentShareRecipients } from './AgentShareRecipients';
import { agentShareUrl, newAgentShareToken } from './agentShareToken';
import './AgentShareLinks.css';
import { RevocationStatus } from './RevocationStatus';
import { UnresolvedRevocations } from './UnresolvedRevocations';
import { retainPendingOperation } from './revocationObservation';
import {
  readRevocations,
  saveRevocation,
  forgetRevocation,
  type RevocationIntent,
} from './revocationIntent';

type Intent = { readonly key: string; readonly body: CreateAgentShareLinkRequest };
const failure = (cause: unknown) =>
  cause instanceof Error ? cause.message : 'The share change could not be confirmed. Try again.';

interface Props {
  readonly client: ResourceViewsClient;
  readonly ownerId: string | undefined;
  readonly agentId: string;
  readonly open: boolean;
  readonly onClose: () => void;
}

export function AgentShareLinksDialog(props: Props) {
  return <AgentShareLinksContent key={JSON.stringify([props.ownerId, props.agentId])} {...props} />;
}

function AgentShareLinksContent({ client, ownerId, agentId, open, onClose }: Props) {
  const [links, setLinks] = useState<readonly AgentShareLink[]>([]);
  const [organizations, setOrganizations] = useState<readonly OrganizationChoice[]>([]);
  const [audience, setAudience] = useState<'public' | 'organization' | 'users'>('public');
  const [organization, setOrganization] = useState('');
  const [people, setPeople] = useState<readonly ShareIdentity[]>([]);
  const [mode, setMode] = useState<'view' | 'view_write'>('view');
  const [error, setError] = useState<string>();
  const [organizationError, setOrganizationError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const loadContext = useMemo(
    () => ({ client, agentId, open, revision }),
    [client, agentId, open, revision],
  );
  const [loaded, setLoaded] = useState<{ context: typeof loadContext; error?: string }>();
  const loading = loaded?.context !== loadContext;
  const loadError = loading ? undefined : loaded.error;
  const [intent, setIntent] = useState<Intent>();
  const [busy, setBusy] = useState<string>();
  const [confirm, setConfirm] = useState<string>();
  const [urls, setUrls] = useState<ReadonlyMap<string, string>>(new Map());
  const [copied, setCopied] = useState<string>();
  const requests = useRef(new Set<AbortController>());
  const revokeKeys = useRef(new Map<string, string>());
  const [completed, setCompleted] = useState<string>();
  const [revocations, setRevocations] = useState<readonly RevocationIntent[]>(() => {
    if (!ownerId) return [];
    try {
      return readRevocations(ownerId, agentId);
    } catch {
      return [];
    }
  });
  const confirmed = useCallback((shareId: string, unresolved = 0) => {
    setCompleted(unresolved > 0 ? undefined : 'Access revoked.');
    revokeKeys.current.delete(shareId);
    setRevocations((rows) => rows.filter((row) => row.shareId !== shareId));
    setLinks((current) => current.filter((item) => item.id !== shareId));
    setUrls((current) => {
      const next = new Map(current);
      next.delete(shareId);
      return next;
    });
    setConfirm(undefined);
  }, []);
  useEffect(() => {
    const activeRequests = requests.current;
    return () => {
      for (const request of activeRequests) request.abort();
    };
  }, []);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(undefined), 2500);
    return () => clearTimeout(timer);
  }, [copied]);
  useEffect(() => {
    if (!open) return;
    const request = new AbortController();
    void Promise.allSettled([
      client.resourceAccess.listAgentShareLinks(agentId, { signal: request.signal }),
      client.resourceAccess.organizations({ signal: request.signal }),
    ])
      .then(([listResult, choicesResult]) => {
        if (request.signal.aborted) return;
        if (listResult.status === 'rejected') throw listResult.reason;
        const list = listResult.value;
        const choices = choicesResult.status === 'fulfilled' ? choicesResult.value : { items: [] };
        setOrganizationError(
          choicesResult.status === 'rejected'
            ? 'Organizations could not be loaded. Reopen sharing to retry.'
            : undefined,
        );
        if (list.items.some((link) => link.agent_id !== agentId))
          throw new Error('Agent sharing identity changed.');
        setLinks(list.items.filter((link) => link.revoked_at === null));
        setOrganizations(choices.items);
        setOrganization((current) =>
          choices.items.some((item) => item.id === current)
            ? current
            : (choices.items[0]?.id ?? ''),
        );
        setLoaded({ context: loadContext });
      })
      .catch((cause: unknown) => {
        if (!request.signal.aborted) setLoaded({ context: loadContext, error: failure(cause) });
      });
    return () => request.abort();
  }, [client, agentId, open, loadContext]);
  const locked = busy !== undefined || intent !== undefined;
  const validAudience =
    audience === 'public' ||
    (audience === 'organization'
      ? organizations.some((item) => item.id === organization)
      : people.length > 0);
  const create = async () => {
    if (requests.current.size > 0 || busy || (!intent && !validAudience)) return;
    const request = new AbortController();
    requests.current.add(request);
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: {
        audience:
          audience === 'public'
            ? { kind: 'public' as const }
            : audience === 'organization'
              ? { kind: 'organization' as const, organization_id: organization }
              : { kind: 'users' as const, subjects: people.map((person) => person.subject) },
        mode,
        token: newAgentShareToken(),
      },
    };
    setIntent(retained);
    setBusy('create');
    setError(undefined);
    try {
      const link = await client.resourceAccess.createAgentShareLink(agentId, retained.body, {
        idempotencyKey: retained.key,
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      if (link.agent_id !== agentId || link.revoked_at !== null)
        throw new Error('The new share link could not be verified.');
      setLinks((current) => [...current.filter((item) => item.id !== link.id), link]);
      setUrls((current) =>
        new Map(current).set(link.id, agentShareUrl(link.id, retained.body.token)),
      );
      setIntent(undefined);
    } catch (cause) {
      if (request.signal.aborted) return;
      if (cause instanceof ApiError && [400, 403, 404, 409, 422].includes(cause.status))
        setIntent(undefined);
      setError(failure(cause));
    } finally {
      requests.current.delete(request);
      if (!request.signal.aborted) setBusy(undefined);
    }
  };
  const revoke = async (link: AgentShareLink) => {
    if (requests.current.size > 0 || busy) return;
    const request = new AbortController();
    requests.current.add(request);
    let retained: RevocationIntent;
    try {
      const prior = ownerId
        ? readRevocations(ownerId, agentId).find((row) => row.shareId === link.id)
        : undefined;
      if (prior?.operation) {
        setRevocations((rows) => [...rows.filter((row) => row.shareId !== link.id), prior]);
        setConfirm(undefined);
        requests.current.delete(request);
        return;
      }
      retained = prior ?? {
        key: revokeKeys.current.get(link.id) ?? crypto.randomUUID(),
        shareId: link.id,
        revision: link.revision,
      };
      if (ownerId) saveRevocation(ownerId, agentId, retained);
      revokeKeys.current.set(link.id, retained.key);
    } catch {
      requests.current.delete(request);
      setError(
        'Your revocation request could not be saved. Restore session storage before retrying.',
      );
      return;
    }
    const key = retained.key;
    const adoptOperation = (operation: string) => {
      const pending = { ...retained, operation };
      setRevocations((rows) => [...rows.filter((row) => row.shareId !== link.id), pending]);
      setConfirm(undefined);
      try {
        if (!ownerId) throw new Error('Owner identity is unavailable.');
        retainPendingOperation({ owner: ownerId, kind: 'agent', id: agentId }, pending, undefined);
      } catch {
        setError(
          'The original request is saved, but status recovery could not be saved. Keep this tab open and restore session storage.',
        );
      }
    };
    setBusy(link.id);
    setError(undefined);
    try {
      const result = await client.resourceAccess.revokeAgentShareLink(
        agentId,
        link.id,
        retained.revision,
        { idempotencyKey: key, signal: request.signal },
      );
      if (request.signal.aborted) return;
      if (result.id !== link.id || result.agent_id !== agentId || result.revoked_at === null)
        throw new Error('Revocation could not be confirmed. Try again.');
      if (result.revocation) {
        adoptOperation(result.revocation.operation);
        return;
      }
      try {
        if (ownerId) forgetRevocation(ownerId, agentId, link.id);
      } catch {
        setError(
          'Access revoked. The saved request could not be cleared; restore session storage before another change.',
        );
      }
      confirmed(link.id);
    } catch (cause) {
      if (!request.signal.aborted) {
        const operation = pendingRevocation(cause, 'agent', agentId);
        if (operation) {
          adoptOperation(operation);
        } else {
          if (cause instanceof ApiError && cause.code === 'resource_access_changed') {
            try {
              if (ownerId) forgetRevocation(ownerId, agentId, link.id);
            } catch {
              setError(
                'The saved request could not be cleared. Restore session storage before retrying.',
              );
              return;
            }
            revokeKeys.current.delete(link.id);
            setRevision((value) => value + 1);
          }
          setError(failure(cause));
        }
      }
    } finally {
      requests.current.delete(request);
      if (!request.signal.aborted) setBusy(undefined);
    }
  };
  const copy = async (id: string, url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(id);
    } catch {
      setError('Copy was unavailable. Select and copy the link shown below.');
    }
  };
  const audienceName = (link: AgentShareLink) =>
    link.audience.kind === 'public'
      ? 'Anyone with the link'
      : link.audience.kind === 'users'
        ? `${String(link.audience.subjects.length)} selected ${link.audience.subjects.length === 1 ? 'person' : 'people'}`
        : organizations.find(
            (item) =>
              item.id ===
              (link.audience.kind === 'organization' ? link.audience.organization_id : ''),
          )?.display_name || 'Organization members';
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Share Agent session"
      description="Create separate links for different people and permissions. Only signed-in recipients can accept a link."
    >
      <div className="agent-share">
        <p className="agent-share__description">
          File access includes this Sandbox’s /home directory and other workspaces within it.
          Managed credentials and runtime control files are excluded. The link shares this Agent
          session; it does not grant Sandbox management.
        </p>
        <form
          className="agent-share__form"
          onSubmit={(event) => {
            event.preventDefault();
            void create();
          }}
        >
          <label>
            <span>Who can accept this link</span>
            <Picker
              label="Share audience"
              value={audience}
              disabled={locked}
              onChange={setAudience}
              options={[
                { value: 'public', label: 'Anyone with the link' },
                { value: 'organization', label: 'Organization members' },
                { value: 'users', label: 'Specific people' },
              ]}
            />
          </label>
          {audience === 'organization' ? (
            <label>
              <span>Organization</span>
              <Picker
                label="Share organization"
                value={organization}
                disabled={locked || organizations.length === 0}
                onChange={setOrganization}
                options={
                  organizations.length
                    ? organizations.map((item) => ({
                        value: item.id,
                        label: item.display_name?.trim() || 'Organization',
                      }))
                    : [{ value: '', label: 'No organizations available' }]
                }
              />
            </label>
          ) : null}
          {audience === 'organization' && organizationError ? (
            <p role="alert">{organizationError}</p>
          ) : null}
          {audience === 'users' ? (
            <AgentShareRecipients
              client={client}
              agentId={agentId}
              value={people}
              onChange={setPeople}
              disabled={locked}
            />
          ) : null}
          <label>
            <span>Permission</span>
            <Picker
              label="Share permission"
              value={mode}
              disabled={locked}
              onChange={setMode}
              options={[
                { value: 'view', label: 'View only' },
                { value: 'view_write', label: 'View and write' },
              ]}
            />
          </label>
          <p className="agent-share__description">
            {mode === 'view'
              ? 'Read the conversation and browse or download files. Messages, settings and files cannot be changed.'
              : 'Send messages, change Agent settings and create, edit or delete files. Recipients cannot reshare the Agent or manage its Sandbox.'}
          </p>
          <Button
            type="submit"
            variant="primary"
            disabled={
              busy !== undefined || loading || Boolean(loadError) || (!intent && !validAudience)
            }
          >
            {busy === 'create' ? 'Creating link…' : intent ? 'Retry creating link' : 'Create link'}
          </Button>
        </form>
        {error ? <p role="alert">{error}</p> : null}
        {completed ? <p role="status">{completed}</p> : null}
        {!loading && !loadError ? (
          <UnresolvedRevocations client={client} owner={ownerId} kind="agent" id={agentId} />
        ) : null}
        {revocations
          .filter((item) => item.operation)
          .map((item) => (
            <RevocationStatus
              key={item.shareId}
              client={client}
              owner={ownerId}
              target={{ kind: 'agent', id: agentId }}
              intent={item}
              onEffective={confirmed}
            />
          ))}
        <section className="agent-share__links" aria-label="Active share links">
          <h3>Active links</h3>
          {loading ? <p role="status">Loading share links…</p> : null}
          {loadError ? (
            <p role="alert">
              {loadError}{' '}
              <Button onClick={() => setRevision((value) => value + 1)}>Retry share links</Button>
            </p>
          ) : null}
          {!loading && !loadError && links.length === 0 ? <p>No active links.</p> : null}
          {links.map((link, index) => (
            <article
              className="agent-share__link"
              key={link.id}
              aria-label={`Share link ${String(index + 1)}`}
            >
              <div className="agent-share__link-summary">
                <strong>{audienceName(link)}</strong>
                <span>{link.mode === 'view' ? 'View only' : 'View and write'}</span>
                <small>
                  {String(link.accepted_count)} accepted · Created{' '}
                  {new Date(link.created_at).toLocaleDateString()}
                </small>
              </div>
              {urls.has(link.id) ? (
                <div className="agent-share__copy">
                  <Input
                    aria-label={`Share link ${String(index + 1)} URL`}
                    readOnly
                    value={urls.get(link.id)!}
                    onFocus={(event) => event.target.select()}
                  />
                  <Button size="sm" onClick={() => void copy(link.id, urls.get(link.id)!)}>
                    {copied === link.id ? 'Copied' : 'Copy link'}
                  </Button>
                </div>
              ) : (
                <p className="agent-share__description">
                  The link secret is not stored. Create a new link if you no longer have it.
                </p>
              )}
              {confirm === link.id ? (
                <div className="agent-share__revoke">
                  <p>Revoke everyone’s access through this link?</p>
                  <Button
                    size="sm"
                    disabled={busy !== undefined}
                    onClick={() => setConfirm(undefined)}
                  >
                    Keep link
                  </Button>
                  <Button size="sm" disabled={busy !== undefined} onClick={() => void revoke(link)}>
                    {busy === link.id ? 'Revoking…' : 'Revoke access'}
                  </Button>
                </div>
              ) : (
                <Button
                  size="sm"
                  disabled={
                    busy !== undefined ||
                    revocations.some((row) => row.shareId === link.id && row.operation)
                  }
                  onClick={() => setConfirm(link.id)}
                >
                  Revoke link
                </Button>
              )}
            </article>
          ))}
        </section>
      </div>
    </Modal>
  );
}
