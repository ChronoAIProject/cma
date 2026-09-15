import { useEffect, useRef, useState } from 'react';
import type { BridgeConfigurationDetail } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { bridgeFailure, useBridgeCommand, type BridgeCommand } from './commands';
import { InlineConfirm } from '../ui/InlineConfirm';

export function BridgeCredential({
  client,
  detail,
  allowed,
  onChanged,
  sharedCommand,
}: {
  readonly client: BridgeViewsClient;
  readonly detail: BridgeConfigurationDetail;
  readonly allowed: boolean;
  readonly onChanged: () => void;
  readonly sharedCommand?: BridgeCommand;
}) {
  const [material, setMaterial] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [confirm, setConfirm] = useState(false);
  const [retainAuthority, setRetainAuthority] = useState(false);
  const [scope, setScope] = useState({
    client,
    id: detail.bridge.id,
    revision: detail.bridge.revision,
    allowed,
  });
  if (
    scope.client !== client ||
    scope.id !== detail.bridge.id ||
    scope.revision !== detail.bridge.revision ||
    scope.allowed !== allowed
  ) {
    setScope({ client, id: detail.bridge.id, revision: detail.bridge.revision, allowed });
    setRetainAuthority(false);
    setMaterial('');
  }
  const active = useRef<AbortController | undefined>(undefined);
  const localCommand = useBridgeCommand(allowed);
  const command = sharedCommand ?? localCommand;
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    if (!allowed) {
      active.current?.abort();
    }
  }, [allowed]);
  useEffect(() => {
    active.current?.abort();
  }, [client, detail.bridge.id, detail.bridge.revision]);
  const status = detail.credential;
  const importKey = async () => {
    if (!allowed || busy || command.locked || material === '') return;
    const request = new AbortController();
    active.current = request;
    setBusy(true);
    setError(undefined);
    const selectedKey = material;
    setMaterial('');
    const timer = setTimeout(() => request.abort(), 15_000);
    try {
      await client.bridges.importKey(detail.bridge.id, detail.bridge.revision, selectedKey, {
        signal: request.signal,
        retainControlPlaneAuthority: retainAuthority,
      });
      request.signal.throwIfAborted();
      onChanged();
    } catch (failure) {
      setError(bridgeFailure(failure));
    } finally {
      clearTimeout(timer);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  return (
    <section className="bridges__fields" aria-labelledby="bridge-credential-title">
      <h2 id="bridge-credential-title">Selected key</h2>
      <p>
        {status.revoked
          ? 'The imported key was revoked.'
          : status.present
            ? 'The selected key is held by CMA.'
            : 'Import the selected key to verify this connection.'}
      </p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void importKey();
        }}
        autoComplete="off"
      >
        <label>
          Import selected Agent key
          <Input
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            maxLength={4096}
            value={allowed ? material : ''}
            disabled={!allowed || busy || command.locked}
            onChange={(event) => setMaterial(event.target.value)}
          />
        </label>
        <p>
          The key is sent once to CMA and cleared from this field. It is never included in a bridge
          detail response.
        </p>
        <label>
          <input
            type="checkbox"
            checked={retainAuthority}
            disabled={!allowed || busy || command.locked}
            onChange={(event) => setRetainAuthority(event.target.checked)}
          />
          Allow this bridge to use my NyxID connection for 30 days
        </label>
        <p>
          This permission applies only to this bridge for{' '}
          {detail.bridge.owner.kind === 'organization'
            ? `organization ${detail.bridge.owner.subject}`
            : `personal account ${detail.bridge.owner.subject}`}
          . It continues after browser logout. CMA checks current account and organization access
          before each operation. Revoke the imported key here to end this permission early. Signing
          in again or expiration requires reconnecting this bridge. Your personal token stays in CMA
          and is never installed in a Sandbox.
        </p>
        <Button type="submit" disabled={!allowed || material === '' || busy || command.locked}>
          {busy ? 'Importing key' : 'Import key'}
        </Button>
      </form>
      {error === undefined ? null : (
        <p role="alert">
          {error} Check the key status before importing the same selected key again.
        </p>
      )}
      {status.present && !status.revoked ? (
        confirm ? (
          <InlineConfirm
            question="Revoke CMA’s custody of this key? Work requiring this key will stop."
            confirmLabel="Revoke imported key"
            onCancel={() => setConfirm(false)}
            busy={busy || command.busy || !allowed}
            onConfirm={() =>
              command.run('Revoke imported key', async (options) => {
                await client.bridges.revokeKey(
                  detail.bridge.id,
                  detail.bridge.revision,
                  status.revision,
                  options,
                );
                options.signal?.throwIfAborted();
                setConfirm(false);
                onChanged();
              })
            }
          />
        ) : (
          <Button disabled={!allowed || busy || command.locked} onClick={() => setConfirm(true)}>
            Revoke imported key
          </Button>
        )
      ) : null}
      {sharedCommand !== undefined || command.error === undefined ? null : (
        <p role="alert">{command.error}</p>
      )}
      {sharedCommand === undefined && command.retryLabel !== undefined && !command.busy ? (
        <Button disabled={!allowed} onClick={command.retry}>
          Retry key revocation
        </Button>
      ) : null}
    </section>
  );
}
