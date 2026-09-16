import { useEffect, useRef, useState } from 'react';
import {
  ApiError,
  ProtocolError,
  type BridgeWorkspaceCleanupRequest,
  type BridgeWorkspaceCleanupStatus,
  type BridgeCleanupAllocation,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';

import { sameCleanupTarget } from './useChildCleanup';

type Intent = {
  readonly allocation: BridgeCleanupAllocation;
  readonly body: BridgeWorkspaceCleanupRequest;
  readonly key: string;
};

/** A refresh may suspend observation, but never replaces an uncertain cleanup intent. */
export function useWorkspaceCleanupRequest({
  client,
  id,
  allowed,
  onAccepted,
  onDenied,
}: {
  readonly client: BridgeViewsClient;
  readonly id: string;
  readonly allowed: boolean;
  readonly onAccepted: () => void;
  readonly onDenied: () => void;
}) {
  const [pending, setPending] = useState<Intent>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string>();
  const [accepted, setAccepted] = useState<BridgeWorkspaceCleanupStatus>();
  const retained = useRef<Intent | undefined>(undefined);
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    if (!allowed) active.current?.abort();
  }, [allowed]);

  const send = async (intent: Intent) => {
    if (!allowed || active.current !== undefined) return;
    const request = new AbortController();
    active.current = request;
    retained.current = intent;
    setPending(intent);
    setBusy(true);
    setMessage(undefined);
    const timeout = setTimeout(() => request.abort(), 15_000);
    try {
      const status = await client.bridges.requestWorkspaceCleanup(
        id,
        intent.allocation.id,
        intent.body,
        {
          signal: request.signal,
          idempotencyKey: intent.key,
        },
      );
      request.signal.throwIfAborted();
      if (!sameCleanupTarget(intent.allocation, status))
        throw new ProtocolError('Workspace cleanup returned a different allocation target.');
      retained.current = undefined;
      setPending(undefined);
      setAccepted(status);
      onAccepted();
    } catch (error) {
      setMessage(
        error instanceof ApiError && [401, 403, 404].includes(error.status)
          ? 'Current Workspace cleanup access is unavailable. The original request is retained until access can be checked.'
          : 'Workspace cleanup acceptance could not be confirmed. Recover the original Workspace request to check its result.',
      );
      if (error instanceof ApiError && [401, 403, 404].includes(error.status)) onDenied();
    } finally {
      clearTimeout(timeout);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  return {
    pending,
    busy,
    message,
    accepted,
    submit: (allocation: BridgeCleanupAllocation, body: BridgeWorkspaceCleanupRequest) => {
      if (retained.current !== undefined) return;
      void send({
        allocation: { ...allocation, target: { ...allocation.target } },
        body: { ...body },
        key: crypto.randomUUID(),
      });
    },
    recover: () => {
      if (retained.current !== undefined) void send(retained.current);
    },
  };
}
