import { useEffect, useRef, useState } from 'react';
import {
  ApiError,
  ProtocolError,
  type BridgeChildCleanupRequest,
  type BridgeChildCleanupStatus,
  type BridgeCleanupAllocation,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';

export function sameCleanupTarget(
  allocation: BridgeCleanupAllocation,
  status: Pick<
    BridgeChildCleanupStatus,
    'bridge_id' | 'allocation_id' | 'generation' | 'allocation_revision' | 'target'
  >,
) {
  return (
    status.bridge_id === allocation.bridge_id &&
    status.allocation_id === allocation.id &&
    status.generation === allocation.generation &&
    status.allocation_revision === allocation.revision &&
    status.target.sandbox_id === allocation.target.sandbox_id &&
    status.target.workspace_id === allocation.target.workspace_id &&
    status.target.worktree_id === allocation.target.worktree_id &&
    status.target.agent_id === allocation.target.agent_id
  );
}

type Intent = {
  readonly allocation: BridgeCleanupAllocation;
  readonly body: BridgeChildCleanupRequest;
  readonly key: string;
};

/** A refresh may suspend observation, but never replaces an uncertain cleanup intent. */
export function useChildCleanup({
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
  const [accepted, setAccepted] = useState<BridgeChildCleanupStatus>();
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
      const status = await client.bridges.requestChildCleanup(
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
        throw new ProtocolError('Cleanup returned a different allocation target.');
      retained.current = undefined;
      setPending(undefined);
      setAccepted(status);
      onAccepted();
    } catch (error) {
      setMessage(
        error instanceof ApiError && [401, 403, 404].includes(error.status)
          ? 'Current cleanup access is unavailable. The original request is retained until access can be checked.'
          : 'Cleanup acceptance could not be confirmed. Recover the original request to check its result.',
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
    submit: (allocation: BridgeCleanupAllocation, revision: number) => {
      if (retained.current !== undefined) return;
      void send({
        allocation: { ...allocation, target: { ...allocation.target } },
        body: {
          expected_bridge_revision: revision,
          expected_allocation_revision: allocation.revision,
          action: 'remove_child',
        },
        key: crypto.randomUUID(),
      });
    },
    recover: () => {
      if (retained.current !== undefined) void send(retained.current);
    },
  };
}
