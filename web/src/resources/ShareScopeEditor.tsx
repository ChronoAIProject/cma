import { useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type ShareReceipt, type ResourceShare } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { CommandKeys } from '../api/client';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { useAuthorizedRead } from './useAuthorizedRead';
import {
  loadSharing,
  matchesShare,
  sharingDeadline,
  shareExposure,
  shareModes,
  type ShareTarget,
} from './sharing';
import { ShareRow } from './ShareRow';
import { refusalSentence } from '../vocabulary/errors';

export type ShareCommand = {
  readonly key: string;
  readonly expected: {
    readonly organization: string;
    readonly mode: ResourceShare['mode'];
    readonly id?: string;
    readonly revision: number;
    readonly revoked: boolean;
  };
  readonly send: (signal: AbortSignal, idempotencyKey: string) => Promise<ShareReceipt>;
};

export function ShareScopeEditor({
  client,
  target,
  keys,
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
  readonly keys: CommandKeys;
}) {
  const { kind, id } = target;
  const read = useMemo(
    () => (signal: AbortSignal) => loadSharing(client, { kind, id, name: '' }, signal),
    [client, kind, id],
  );
  const sharing = useAuthorizedRead(read);
  const organizationRead = useMemo(
    () => async (signal: AbortSignal) => ({
      value: (await client.resourceAccess.organizations({ signal })).items,
    }),
    [client],
  );
  const organizations = useAuthorizedRead(organizationRead);
  const choices = organizations.state.status === 'ready' ? organizations.state.value : [];
  const shares = sharing.state.status === 'ready' ? sharing.state.value : undefined;
  const [organization, setOrganization] = useState('');
  const [mode, setMode] = useState<ResourceShare['mode']>('view');
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const active = useRef<AbortController | undefined>(undefined);
  const name = (org: string) =>
    choices.find((choice) => choice.id === org)?.display_name?.trim() ||
    'Organization name unavailable';
  const available = shares !== undefined;
  useEffect(() => {
    if (!available) active.current?.abort();
  }, [available]);
  useEffect(() => () => active.current?.abort(), [client, kind, id]);
  const run = async (command: ShareCommand) => {
    if (active.current !== undefined || !available) return;
    const request = new AbortController();
    active.current = request;
    setBusy(true);
    setFailure(undefined);
    setNotice(undefined);
    const started = Date.now();
    const timer = setTimeout(() => {
      request.abort();
      if (active.current === request) {
        setBusy(false);
        setFailure(
          'The result could not be confirmed. Retry the same action to recover its result.',
        );
        active.current = undefined;
      }
    }, 15_000);
    try {
      const access = await client.resourceAccess.effective(kind, id, { signal: request.signal });
      sharingDeadline(access, started);
      request.signal.throwIfAborted();
      const receipt = await command.send(request.signal, keys.for(`${kind}:${id}:${command.key}`));
      request.signal.throwIfAborted();
      const expected = command.expected;
      if (
        !matchesShare(receipt.share, target) ||
        receipt.share.target_org_id !== expected.organization ||
        receipt.share.mode !== expected.mode ||
        (expected.id !== undefined && receipt.share.id !== expected.id) ||
        receipt.share.revision !== expected.revision ||
        (receipt.share.revoked_at !== null) !== expected.revoked
      )
        throw new Error(
          'Sharing response did not confirm the requested change. Check current sharing before retrying.',
        );
      keys.settle(`${kind}:${id}:${command.key}`);
      setNotice('Sharing command confirmed. Current grants are shown below.');
      setAcknowledged(false);
      sharing.reload();
    } catch (error) {
      if (!request.signal.aborted) {
        setFailure(
          error instanceof ApiError
            ? refusalSentence(error)
            : error instanceof Error
              ? error.message
              : 'Sharing could not be updated. Retry the same action.',
        );
        sharing.reload();
      }
    } finally {
      clearTimeout(timer);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  const selected = choices.find((choice) => choice.id === organization);
  const existing = shares?.some((share) => share.target_org_id === organization);
  return (
    <div className="resource-sharing">
      <h3>Whole Sandbox · {target.name}</h3>
      <p>This scope includes every current and future Workspace and Agent in the Sandbox.</p>
      <Button
        size="sm"
        disabled={busy}
        onClick={() => {
          sharing.reload();
          organizations.reload();
        }}
      >
        Refresh sharing
      </Button>
      {failure === undefined ? null : <p role="alert">{failure}</p>}
      {notice === undefined ? null : <p role="status">{notice}</p>}
      {sharing.state.status === 'loading' ? (
        <p role="status">Checking current sharing access</p>
      ) : null}
      {sharing.state.status === 'error' ? <p role="alert">{sharing.state.message}</p> : null}
      {shares === undefined ? null : (
        <>
          <h3>Current grants</h3>
          {shares.length === 0 ? (
            <p>No direct organization grants.</p>
          ) : (
            <ul className="resource-sharing__grants">
              {shares.map((share) => (
                <ShareRow
                  key={`${share.id}:${share.revision}`}
                  share={share}
                  name={name(share.target_org_id)}
                  target={target}
                  client={client}
                  busy={busy}
                  onCommand={run}
                />
              ))}
            </ul>
          )}
          <h3>Share with an organization</h3>
          {organizations.state.status === 'loading' ? (
            <p role="status">Loading organization choices</p>
          ) : null}
          {organizations.state.status === 'error' ? (
            <p role="alert">
              Organization choices are unavailable. Existing grants can still be revoked.
            </p>
          ) : null}
          {organizations.state.status === 'ready' && choices.length === 0 ? (
            <p>No current organizations are available for a new share.</p>
          ) : null}
          <Picker
            label="Share with organization"
            value={organization}
            options={[
              {
                value: '',
                label: 'Choose organization',
                unavailable: 'Select a current organization.',
              },
              ...choices.map((choice) => ({
                value: choice.id,
                label: choice.display_name?.trim() || 'Organization name unavailable',
              })),
            ]}
            disabled={busy || choices.length === 0}
            onChange={(value) => {
              setOrganization(value);
              setAcknowledged(false);
            }}
          />
          <Picker
            label="New share access"
            value={mode}
            options={shareModes}
            disabled={busy}
            onChange={(value) => {
              setMode(value);
              setAcknowledged(false);
            }}
          />
          <div className="resource-sharing__preview" aria-label="Sharing exposure preview">
            <p>{shareExposure(mode)}</p>
          </div>
          <label className="resource-sharing__acknowledgement">
            <input
              type="checkbox"
              checked={acknowledged}
              disabled={busy}
              onChange={(event) => setAcknowledged(event.target.checked)}
            />
            I intend to share the whole Sandbox, including its current and future resources.
          </label>
          {existing ? (
            <p>
              This organization already has a direct grant. Use its controls above to change access.
            </p>
          ) : null}
          <Button
            variant="primary"
            disabled={busy || selected === undefined || existing || !acknowledged}
            onClick={() =>
              void run({
                key: `create:${organization}:${mode}`,
                expected: { organization, mode, revision: 1, revoked: false },
                send: (signal, idempotencyKey) =>
                  client.resourceAccess.createShare(
                    kind,
                    id,
                    { target_org_id: organization, mode },
                    { signal, idempotencyKey },
                  ),
              })
            }
          >
            Create share
          </Button>
        </>
      )}
    </div>
  );
}
