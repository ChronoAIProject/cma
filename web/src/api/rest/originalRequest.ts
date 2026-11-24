import type { Cma } from '@cma/client';
import type { ReadOptions } from '../client';
import type { OriginalRequestRead } from '../originalRequest';

export async function readOriginalRequest(
  sdk: Cma,
  agentId: string,
  options?: ReadOptions,
): Promise<OriginalRequestRead> {
  const value = await sdk.agents.originalRequest(agentId, options);
  return value.status === 'unavailable'
    ? { kind: 'unavailable', agentId: value.agent_id }
    : {
        kind: 'ready',
        agentId: value.agent_id,
        text: value.original_request.text,
        source: 'admitted-first-task',
        turnSequence: value.original_request.turn_sequence,
      };
}
