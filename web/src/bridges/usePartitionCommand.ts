import { useEffect, useRef, useState } from 'react';
import { ApiError, ProtocolError, type BridgePartition } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';

export type PartitionAction = 'reset' | 'detach';
type Receipt = Awaited<ReturnType<BridgeViewsClient['bridges']['resetPartition']>>;
type Request = Parameters<BridgeViewsClient['bridges']['resetPartition']>[2];
type Intent = {
  readonly partition: BridgePartition;
  readonly action: PartitionAction;
  readonly body: Request;
  readonly key: string;
};

/** The original command survives a refresh or uncertain reply in this viewer scope. */
export function usePartitionCommand({
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
  const [accepted, setAccepted] = useState<Receipt>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string>();
  const retained = useRef<Intent | undefined>(undefined);
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    if (!allowed) active.current?.abort();
  }, [allowed]);
  async function send(intent: Intent) {
    if (!allowed || active.current !== undefined) return;
    const request = new AbortController();
    active.current = request;
    retained.current = intent;
    setPending(intent);
    setBusy(true);
    setMessage(undefined);
    const timeout = setTimeout(() => request.abort(), 15_000);
    try {
      const options = { signal: request.signal, idempotencyKey: intent.key };
      const receipt =
        intent.action === 'reset'
          ? await client.bridges.resetPartition(id, intent.partition.id, intent.body, options)
          : await client.bridges.detachPartition(id, intent.partition.id, intent.body, options);
      request.signal.throwIfAborted();
      if (
        receipt.bridge_id !== id ||
        receipt.allocation_id !== intent.partition.id ||
        receipt.generation !== intent.partition.generation ||
        receipt.action !== intent.action ||
        receipt.previous_revision !== intent.body.expected_revision ||
        receipt.expected_bridge_revision !== intent.body.expected_bridge_revision
      )
        throw new ProtocolError('Partition command returned a different original request.');
      retained.current = undefined;
      setPending(undefined);
      setAccepted(receipt);
      onAccepted();
    } catch (error) {
      const denied = error instanceof ApiError && [401, 403, 404].includes(error.status);
      setMessage(
        denied
          ? 'Current partition management access is unavailable. The original request is retained until access can be checked.'
          : 'Partition command acceptance could not be confirmed. Recover the original request to check its result.',
      );
      if (denied) onDenied();
    } finally {
      clearTimeout(timeout);
      if (active.current === request) {
        active.current = undefined;
        setBusy(false);
      }
    }
  }
  return {
    pending,
    accepted,
    busy,
    message,
    submit: (partition: BridgePartition, action: PartitionAction, revision: number) => {
      if (retained.current !== undefined) return;
      setAccepted(undefined);
      void send({
        partition: structuredClone(partition),
        action,
        body: { expected_bridge_revision: revision, expected_revision: partition.revision },
        key: crypto.randomUUID(),
      });
    },
    recover: () => {
      if (retained.current !== undefined) void send(retained.current);
    },
  };
}
export type PartitionCommandController = ReturnType<typeof usePartitionCommand>;
