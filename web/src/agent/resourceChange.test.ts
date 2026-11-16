import { expect, it } from 'vitest';
import { loadAgent } from './loadAgent';
import { agentClient } from '../test/agent';
import { affectsCurrentAgent, type ResourceChange } from './resourceChange';

it.each<ResourceChange>([
  { kind: 'pause_sandbox', sandboxId: 'sbx_1' },
  { kind: 'revive_sandbox', sandboxId: 'sbx_1' },
  { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
  { kind: 'remove_workspace', workspaceId: 'wsp_1' },
  { kind: 'stop_agent', agentId: 'agt_1' },
  { kind: 'delete_agent', agentId: 'agt_1' },
])('matches only the exact current lineage for $kind', async (target) => {
  const ready = await loadAgent(agentClient(), 'agt_1', new AbortController().signal);
  expect(affectsCurrentAgent(target, { status: 'ready', ...ready })).toBe(true);
  const unrelated = {
    ...target,
    ...('agentId' in target
      ? { agentId: 'other' }
      : 'workspaceId' in target
        ? { workspaceId: 'other' }
        : { sandboxId: 'other' }),
  };
  expect(affectsCurrentAgent(unrelated, { status: 'ready', ...ready })).toBe(false);
  for (const status of ['idle', 'loading'] as const)
    expect(affectsCurrentAgent(target, { status })).toBe(false);
  expect(affectsCurrentAgent(target, { status: 'error', message: 'Unavailable' })).toBe(false);
});
