import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ApiError,
  type EffectiveAccess,
  type SandboxCommandKind,
  type SandboxCommandReceipt,
  type SandboxCommandRequest,
} from '@cma/client';
import { CommandKeys } from '../api/client';
import type { ResourceViewsClient } from './client';

interface Intent {
  readonly body: SandboxCommandRequest;
  readonly key: string;
  readonly receipt?: SandboxCommandReceipt;
  readonly message?: string;
  readonly refused?: boolean;
}
interface Basis {
  readonly revision?: number;
  readonly access: EffectiveAccess;
}
// Keep uncertain intents within the mounted viewer console across shared navigation.
// No credentials or owner configuration are persisted in browser storage.
const retained = new WeakMap<object, Map<string, Intent>>();
export const controlCapability = (kind: SandboxCommandKind) =>
  kind === 'retire' ? 'manage_sharing' : 'manage_sandbox';

export function useSandboxControl(
  client: ResourceViewsClient | undefined,
  id: string,
  basis: Basis | undefined,
  refresh: () => void,
  retentionScope: object,
) {
  const scope = useMemo(() => ({ client, id, retentionScope }), [client, id, retentionScope]);
  const [stored, setStored] = useState(() => ({
    scope,
    intent: client && retained.get(retentionScope)?.get(id),
  }));
  if (stored.scope !== scope)
    setStored({ scope, intent: client && retained.get(retentionScope)?.get(id) });
  const intent = stored.scope === scope ? stored.intent : undefined;
  const work = useRef<AbortController | undefined>(undefined);
  const [running, setRunning] = useState<AbortController>();
  const busy = running !== undefined && !running.signal.aborted;
  const publish = useCallback(
    (next: Intent | undefined) => {
      if (client) {
        const entries = retained.get(retentionScope) ?? new Map<string, Intent>();
        if (next) entries.set(id, next);
        else entries.delete(id);
        retained.set(retentionScope, entries);
      }
      setStored((current) => (current.scope === scope ? { scope, intent: next } : current));
    },
    [client, id, scope, retentionScope],
  );
  const available = basis !== undefined;
  useEffect(() => {
    if (!available) {
      work.current?.abort();
      work.current = undefined;
    }
    return () => {
      work.current?.abort();
      work.current = undefined;
    };
  }, [scope, available]);

  const submit = async (next: Intent) => {
    if (
      !client ||
      !basis ||
      work.current ||
      !basis.access.capabilities.includes(controlCapability(next.body.command))
    )
      return;
    const request = new AbortController();
    work.current = request;
    setRunning(request);
    // Admission may succeed even when its response is lost or aborted.
    publish({ ...next, message: 'The result is not confirmed. Retry to recover this request.' });
    const timeout = setTimeout(() => request.abort(), 15_000);
    try {
      const started = Date.now();
      const access = await client.resourceAccess.effective('sandbox', id, {
        signal: request.signal,
      });
      request.signal.throwIfAborted();
      const deadline =
        access.revalidate_at == null
          ? started + 55_000
          : Math.min(started + 55_000, Date.parse(access.revalidate_at));
      if (
        !access.capabilities.includes(controlCapability(next.body.command)) ||
        access.policy_revision !== basis.access.policy_revision ||
        !Number.isFinite(deadline) ||
        deadline <= Date.now()
      ) {
        refresh();
        throw new Error('Sandbox access changed. Refresh access before retrying this request.');
      }
      const receipt = await client.sandboxes.submitCommand(id, next.body, {
        signal: request.signal,
        idempotencyKey: next.key,
      });
      request.signal.throwIfAborted();
      publish({ ...next, receipt, message: undefined });
      if (receipt.state.status !== 'queued') refresh();
    } catch (error) {
      if (!request.signal.aborted) {
        publish({
          ...next,
          refused:
            error instanceof ApiError &&
            [
              'invalid_sandbox_command',
              'sandbox_command_conflict',
              'sandbox_command_busy',
            ].includes(error.code ?? ''),
          message:
            error instanceof ApiError && error.status < 500
              ? 'This request was refused. Check current access and status before retrying.'
              : 'The result is not confirmed. Retry to recover this request.',
        });
        if (error instanceof ApiError && [401, 403, 404, 409, 503].includes(error.status))
          refresh();
      }
    } finally {
      clearTimeout(timeout);
      if (work.current === request) work.current = undefined;
      request.abort();
      setRunning((current) => (current === request ? undefined : current));
    }
  };

  useEffect(() => {
    if (!client || !available || intent?.receipt?.state.status !== 'queued') return;
    const request = new AbortController();
    let active = true;
    const timer = setTimeout(() => {
      if (work.current) return;
      work.current = request;
      const timeout = setTimeout(() => request.abort(), 15_000);
      void client.sandboxes
        .readCommand(intent.receipt!.command_id, { signal: request.signal })
        .then((receipt) => {
          request.signal.throwIfAborted();
          if (
            receipt.sandbox_id !== id ||
            receipt.command !== intent.body.command ||
            receipt.expected_revision !== intent.body.expected_revision
          )
            throw new Error('The receipt no longer matches this Sandbox request.');
          publish({ ...intent, receipt, message: undefined });
          if (receipt.state.status !== 'queued') refresh();
        })
        .catch((error: unknown) => {
          if (active) {
            publish({
              ...intent,
              message: 'The request is saved. Its current result could not be checked.',
            });
            if (error instanceof ApiError && [401, 403, 404, 409, 503].includes(error.status))
              refresh();
          }
        })
        .finally(() => {
          clearTimeout(timeout);
          if (work.current === request) work.current = undefined;
        });
    }, 2_000);
    return () => {
      active = false;
      clearTimeout(timer);
      request.abort();
      if (work.current === request) work.current = undefined;
    };
  }, [client, id, available, intent, publish, refresh]);

  return {
    intent,
    busy,
    start: (command: SandboxCommandKind) => {
      if (
        intent ||
        basis?.revision === undefined ||
        !Number.isSafeInteger(basis.revision) ||
        basis.revision <= 0
      )
        return;
      void submit({
        body: { command, expected_revision: basis.revision },
        key: new CommandKeys().for('sandbox-control'),
      });
    },
    retry: () => {
      if (intent) void submit(intent);
    },
    dismiss: () => {
      if (
        (intent?.refused || (intent?.receipt && intent.receipt.state.status !== 'queued')) &&
        !work.current
      )
        publish(undefined);
    },
  };
}
