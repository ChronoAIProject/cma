import type { PartitionCommandAction, PartitionCommandReceipt } from '../src/index.js';
export const bridge = 'brg_partition';
export const allocation = `bral_${'1'.repeat(64)}`;
export const successor = `bral_${'2'.repeat(64)}`;
export const request = { expected_bridge_revision: 5, expected_revision: 3 };
export function receipt(action: PartitionCommandAction = 'reset'): PartitionCommandReceipt {
  return {
    id: `bpc_${'3'.repeat(64)}`,
    bridge_id: bridge,
    request_sha256: '4'.repeat(64),
    actor: { kind: 'human', subject: 'owner' },
    action,
    expected_bridge_revision: 5,
    allocation_id: allocation,
    generation: 1,
    previous_revision: 3,
    result: { id: allocation, generation: 1, revision: 4, state: 'retired' },
    successor:
      action === 'reset' ? { id: successor, generation: 2, revision: 1, state: 'ready' } : null,
    created_at_ms: 1000,
  };
}
