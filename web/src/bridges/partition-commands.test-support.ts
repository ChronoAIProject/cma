import { vi } from 'vitest';
import type {
  BridgePartition,
  PartitionCommandAction,
  PartitionCommandReceipt,
} from '@chronoai/cma-sdk';
import { bridgeTransport } from './fixtures.test-support';
export const allocationId = `bral_${'1'.repeat(64)}`;
export const nextId = `bral_${'2'.repeat(64)}`;
export function partitionCommandFixture() {
  const partition: BridgePartition = {
    id: allocationId,
    bridge_id: 'brg_one',
    generation: 1,
    revision: 3,
    config_revision: 1,
    current: true,
    state: 'ready',
    placement: 'existing_agent',
    context_sharing: 'fixed_agent',
    target: {
      status: 'available',
      ids: {
        sandbox_id: 'sbx_exact',
        workspace_id: 'wks_exact',
        worktree_id: 'wkt_exact',
        agent_id: 'agt_exact',
      },
      present: ['sandbox', 'workspace', 'worktree', 'agent'],
    },
    source: {
      status: 'available',
      platform: 'telegram',
      chat_id: 'chat-exact',
      sender_id: 'sender-exact',
      sender_display_name: null,
      conversation_type: 'private',
    },
    observed_at_ms: 1000,
  };
  const state = {
    partition,
    loseReply: false,
    pause: undefined as Promise<Response> | undefined,
    accepted: undefined as PartitionCommandReceipt | undefined,
  };
  const writes: { path: string; key: string | null; body: unknown }[] = [];
  const transport = bridgeTransport((url, init) => {
    if (init.method === 'POST' && /\/(reset|detach)$/.test(url.pathname)) {
      writes.push({
        path: url.pathname,
        key: new Headers(init.headers).get('idempotency-key'),
        body: JSON.parse(String(init.body)),
      });
      const action = url.pathname.endsWith('/detach') ? 'detach' : 'reset';
      state.accepted ??= result(action);
      state.partition = {
        ...state.partition,
        state: 'retired',
        revision: 4,
        current: action === 'detach',
      };
      if (state.pause !== undefined) return state.pause;
      if (state.loseReply) {
        state.loseReply = false;
        return Promise.reject(new TypeError('reply lost'));
      }
      return Response.json(state.accepted, { status: 202 });
    }
    if (url.pathname.endsWith(`/partitions/${allocationId}`))
      return Response.json({
        partition: state.partition,
        activity: {
          status: 'available',
          agent_status: 'ready',
          pending: [],
          pending_truncated: false,
          source_unavailable: false,
          latest: null,
          observed_at_ms: 1000,
        },
      });
    if (url.pathname.endsWith('/partitions'))
      return Response.json({ items: [state.partition], next_cursor: null });
    return undefined;
  });
  return {
    ...transport,
    state,
    writes,
    props: {
      client: transport.client,
      viewerId: 'owner',
      id: 'brg_one',
      ownerScope: 'person:owner',
      revision: 5,
      allowed: true,
      onNavigate: vi.fn(),
      onAccessChanged: vi.fn(),
    },
  };
}
export function result(action: PartitionCommandAction = 'reset'): PartitionCommandReceipt {
  return {
    id: `bpc_${'3'.repeat(64)}`,
    bridge_id: 'brg_one',
    request_sha256: '4'.repeat(64),
    actor: { kind: 'human', subject: 'owner' },
    action,
    expected_bridge_revision: 5,
    allocation_id: allocationId,
    generation: 1,
    previous_revision: 3,
    result: { id: allocationId, generation: 1, revision: 4, state: 'retired' },
    successor:
      action === 'reset' ? { id: nextId, generation: 2, revision: 1, state: 'ready' } : null,
    created_at_ms: 1000,
  };
}
