import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import { CommandKeys, type CmaWebClient } from '../api/client';
import type { SandboxRuntimeUpdate } from '../api/types';

type Receipt = { readonly runtimeUpdate: SandboxRuntimeUpdate; readonly revision: number };
interface Intent {
  readonly target: string;
  readonly key: string;
  readonly receipt?: Receipt;
  readonly message?: string;
  readonly refused?: boolean;
}

// Keep exact unresolved requests through navigation within this authenticated client.
// Neither credentials nor these requests are written to browser storage.
const retained = new WeakMap<CmaWebClient, Map<string, Intent>>();

export function useRuntimeUpdate(
  client: CmaWebClient,
  sandboxId: string,
  ownerId: string,
  revision: number,
  permitted: boolean,
  onAccepted: (receipt: Receipt) => void,
) {
  const scope = useMemo(
    () => ({ client, sandboxId, ownerId, key: JSON.stringify([ownerId, sandboxId]) }),
    [client, sandboxId, ownerId],
  );
  const [stored, setStored] = useState(() => ({
    scope,
    intent: retained.get(client)?.get(scope.key),
  }));
  if (stored.scope !== scope) setStored({ scope, intent: retained.get(client)?.get(scope.key) });
  const storedIntent = stored.scope === scope ? stored.intent : undefined;
  const observed = storedIntent?.receipt !== undefined && revision >= storedIntent.receipt.revision;
  const intent = observed ? undefined : storedIntent;
  const work = useRef<AbortController | undefined>(undefined);
  const [running, setRunning] = useState<AbortController>();
  const publish = useCallback(
    (next: Intent | undefined) => {
      const entries = retained.get(client) ?? new Map<string, Intent>();
      if (next) entries.set(scope.key, next);
      else entries.delete(scope.key);
      retained.set(client, entries);
      setStored((current) => (current.scope === scope ? { scope, intent: next } : current));
    },
    [client, scope],
  );
  useEffect(() => {
    return () => {
      work.current?.abort();
      work.current = undefined;
    };
  }, [scope, permitted]);
  useEffect(() => {
    if (observed && retained.get(client)?.get(scope.key) === storedIntent)
      retained.get(client)?.delete(scope.key);
  }, [client, scope, storedIntent, observed]);

  const submit = async (next: Intent) => {
    if (!permitted || !client.updateSandboxRuntime || work.current) return;
    const request = new AbortController();
    work.current = request;
    setRunning(request);
    publish({
      ...next,
      refused: false,
      message: 'The update request is not yet confirmed. Retry to check this same request.',
    });
    const timeout = setTimeout(() => request.abort(), 15_000);
    try {
      const receipt = await client.updateSandboxRuntime(sandboxId, next.target, {
        idempotencyKey: next.key,
        signal: request.signal,
      });
      request.signal.throwIfAborted();
      publish({ ...next, receipt });
      onAccepted(receipt);
    } catch (error) {
      if (!request.signal.aborted) {
        const refused =
          error instanceof ApiError && error.status < 500 && ![408, 429].includes(error.status);
        publish({
          ...next,
          refused,
          message: refused
            ? error.status === 409
              ? 'The available runtime or Sandbox status changed. Review the latest status before requesting an update again.'
              : error.message
            : 'The update request is not yet confirmed. Retry to check this same request.',
        });
      }
    } finally {
      clearTimeout(timeout);
      if (work.current === request) work.current = undefined;
      setRunning((current) => (current === request ? undefined : current));
    }
  };

  return {
    intent,
    busy: running !== undefined && !running.signal.aborted,
    start: (target: string) => {
      if (intent && !intent.refused) return;
      void submit({ target, key: new CommandKeys().for('runtime-update') });
    },
    retry: () => {
      if (intent && !intent.receipt && !intent.refused) void submit(intent);
    },
  };
}
