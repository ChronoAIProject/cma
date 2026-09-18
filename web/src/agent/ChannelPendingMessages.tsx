import { useEffect, useMemo, useRef, useState } from 'react';
import type { PendingChannelMessages } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import type { ProvenanceState } from './provenance';

type PendingSnapshot = Extract<PendingChannelMessages, { status: 'available' }>;
const isVisible = () => document.visibilityState !== 'hidden';

/** Private pending previews are ephemeral; only durable replay adds transcript messages. */
export function ChannelPendingMessages({
  agentId,
  viewerId,
  provenance,
  cursor,
  enabled,
}: {
  readonly agentId: string;
  readonly viewerId?: string;
  readonly provenance: ProvenanceState;
  readonly cursor: number;
  readonly enabled: boolean;
}) {
  const client = useCmaClient().bridgeViews;
  const channel =
    provenance.status === 'ready' &&
    provenance.agentId === agentId &&
    provenance.value.creation_origin === 'channel_bridge'
      ? provenance.value.details?.channel_bridge
      : undefined;
  const bridge = channel?.bridge_id;
  const partition = channel?.partition_id;
  const generation = channel?.generation;
  const context = useMemo(
    () => ({ agentId, viewerId, client, bridge, partition, generation, enabled }),
    [agentId, viewerId, client, bridge, partition, generation, enabled],
  );
  const [observed, setObserved] = useState<{
    context: typeof context;
    snapshot: PendingSnapshot;
  }>();
  const refresh = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    if (
      !enabled ||
      client === undefined ||
      viewerId === undefined ||
      bridge === undefined ||
      partition === undefined ||
      generation === undefined
    )
      return;
    const lifetime = new AbortController();
    let request: AbortController | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let retry = 1000;
    const poll = async () => {
      if (request !== undefined || lifetime.signal.aborted || !isVisible()) return;
      clearTimeout(timer);
      const current = new AbortController();
      request = current;
      try {
        const snapshot = await client.bridges.pendingMessages(bridge, partition, {
          signal: current.signal,
        });
        if (lifetime.signal.aborted || current.signal.aborted) return;
        if (snapshot.status !== 'available') setObserved(undefined);
        else {
          if (
            snapshot.agent_id !== agentId ||
            snapshot.bridge_id !== bridge ||
            snapshot.partition_id !== partition ||
            snapshot.generation !== generation
          )
            throw new Error('Channel message observation changed its target.');
          setObserved({ context, snapshot });
        }
        retry = 1000;
      } catch {
        if (!lifetime.signal.aborted && !current.signal.aborted) {
          setObserved(undefined);
          retry = Math.min(retry * 2, 10000);
        }
      } finally {
        if (request === current) request = undefined;
        if (!lifetime.signal.aborted && isVisible()) timer = setTimeout(() => void poll(), retry);
      }
    };
    const foreground = () => {
      if (!isVisible()) {
        request?.abort();
        clearTimeout(timer);
        setObserved(undefined);
      } else void poll();
    };
    refresh.current = () => void poll();
    void poll();
    document.addEventListener('visibilitychange', foreground);
    return () => {
      lifetime.abort();
      request?.abort();
      clearTimeout(timer);
      refresh.current = undefined;
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [context, agentId, viewerId, client, bridge, partition, generation, enabled]);
  const snapshot = observed?.context === context ? observed.snapshot : undefined;
  // A stream can overtake an in-flight poll. Never display a preview from an
  // older durable head beside the same message's newly started transcript.
  const stale = snapshot !== undefined && cursor > snapshot.through_sequence;
  useEffect(() => {
    if (stale) refresh.current?.();
  }, [stale, cursor]);
  if (!enabled || snapshot === undefined || stale || snapshot.messages.length === 0) return null;
  return (
    <section
      className="agent-channel-pending"
      aria-label="Channel messages waiting to start"
      aria-live="polite"
    >
      {snapshot.messages.map((message) => (
        <div className="agent-action-error" role="status" key={message.command_id}>
          <div>
            <strong>
              {message.platform.startsWith('telegram') ? 'Telegram' : 'Channel'} message received ·
              Waiting to start
            </strong>
            <p className="agent-channel-pending__text">
              {message.text || 'Message received.'}
              {message.text_truncated ? '…' : ''}
            </p>
          </div>
        </div>
      ))}
      {snapshot.pending_truncated ? (
        <p role="status">More channel messages are waiting to start.</p>
      ) : null}
    </section>
  );
}
