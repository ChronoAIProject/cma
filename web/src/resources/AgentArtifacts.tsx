import { useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type AgentArtifactMetadata, type EffectiveAccess } from '@cma/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { Button } from '../ui/Button';
import './AgentArtifacts.css';

function deadline(access: EffectiveAccess, started: number) {
  const value = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  if (
    !access.capabilities.includes('view_artifacts') ||
    !Number.isFinite(value) ||
    value <= Date.now()
  )
    throw new Error('Current access does not allow these artifacts.');
  return value;
}
export async function authorizedArtifacts<Value>(
  client: ResourceViewsClient,
  id: string,
  signal: AbortSignal,
  read: () => Promise<Value>,
) {
  const started = Date.now();
  const initial = await client.resourceAccess.effective('agent', id, { signal });
  const expires = deadline(initial, started);
  signal.throwIfAborted();
  const value = await read();
  signal.throwIfAborted();
  const current = await client.resourceAccess.effective('agent', id, { signal });
  signal.throwIfAborted();
  const finalDeadline = Math.min(expires, deadline(current, started));
  if (current.policy_revision !== initial.policy_revision || finalDeadline <= Date.now())
    throw new Error('Artifact access changed. Refresh to check current access.');
  return { value, revalidateAt: new Date(finalDeadline).toISOString() };
}

function saveArtifact(bytes: Uint8Array, artifact: AgentArtifactMetadata) {
  const url = URL.createObjectURL(
    new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' }),
  );
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = artifact.name;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function AgentArtifacts({
  client,
  id,
  onAccessChanged,
  enabled = true,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly onAccessChanged?: () => void;
  readonly enabled?: boolean;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () =>
      !enabled
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return await authorizedArtifacts(client, id, signal, () =>
                client.agents.listArtifacts(id, { cursor, limit: 20, signal }),
              );
            } catch (error) {
              if (
                !signal.aborted &&
                error instanceof ApiError &&
                [401, 403, 404, 409].includes(error.status)
              )
                onAccessChanged?.();
              throw error;
            }
          },
    [client, id, cursor, onAccessChanged, enabled],
  );
  const current = useAuthorizedRead(read);
  const pending = useRef<AbortController | undefined>(undefined);
  const [download, setDownload] = useState<{ read: typeof read; id: string; message?: string }>();
  useEffect(() => () => pending.current?.abort(), [read]);
  useEffect(() => {
    if (current.state.status !== 'ready') pending.current?.abort();
  }, [current.state.status]);
  const run = async (artifact: AgentArtifactMetadata) => {
    if (!enabled) return;
    pending.current?.abort();
    const request = new AbortController();
    pending.current = request;
    setDownload({ read, id: artifact.id });
    const timer = setTimeout(() => request.abort(), 15_000);
    try {
      const result = await authorizedArtifacts(client, id, request.signal, async () => {
        const bytes = await client.agents.downloadArtifact(id, artifact.id, {
          signal: request.signal,
        });
        const hash = await crypto.subtle.digest('SHA-256', new Uint8Array(bytes));
        const digest = [...new Uint8Array(hash)]
          .map((byte) => byte.toString(16).padStart(2, '0'))
          .join('');
        if (bytes.length !== artifact.size_bytes || digest !== artifact.sha256)
          throw new Error('Artifact content did not match its published metadata.');
        return bytes;
      });
      request.signal.throwIfAborted();
      saveArtifact(result.value, artifact);
      setDownload(undefined);
    } catch (error) {
      if (!request.signal.aborted) {
        setDownload({
          read,
          id: artifact.id,
          message: error instanceof Error ? error.message : 'Artifact download failed.',
        });
        current.reload();
        if (error instanceof ApiError && [401, 403, 404, 409].includes(error.status))
          onAccessChanged?.();
      } else if (pending.current === request) {
        setDownload({
          read,
          id: artifact.id,
          message: 'Artifact download was interrupted. Try again.',
        });
      }
    } finally {
      clearTimeout(timer);
      if (pending.current === request) pending.current = undefined;
    }
  };
  const active = download?.read === read ? download : undefined;
  return (
    <section className="agent-artifacts" aria-label="Approved Agent artifacts">
      <p>Files explicitly published for this Agent’s readers.</p>
      <Button size="sm" disabled={!enabled} onClick={current.reload}>
        Refresh artifacts
      </Button>
      {active?.message === undefined ? null : <p role="alert">{active.message}</p>}
      {current.state.status === 'loading' ? (
        <p role="status">Loading approved artifacts</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>No published artifacts on this page.</p>
          ) : (
            <ul>
              {current.state.value.items.map((artifact) => (
                <li key={artifact.id}>
                  <span>{artifact.name}</span>
                  <span>{artifact.size_bytes.toLocaleString()} bytes</span>
                  <Button
                    size="sm"
                    disabled={active !== undefined && active.message === undefined}
                    onClick={() => void run(artifact)}
                  >
                    Download {artifact.name}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          <div className="agent-artifacts__pages">
            <Button
              size="sm"
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous artifacts
            </Button>
            <Button
              size="sm"
              disabled={current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status === 'ready') {
                  const next = current.state.value.next;
                  if (next !== undefined) setCursors((values) => [...values, next]);
                }
              }}
            >
              Next artifacts
            </Button>
          </div>
        </>
      )}
    </section>
  );
}
