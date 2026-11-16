import { expect, it } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript } from './conversation';
import type { AgentResourceState } from './controllerTypes';
import { controlBaselineReady } from './controlReadiness';
import { harness } from './transport/controller.fixture';

const ready = {
  status: 'ready',
  detail: AGENT_DETAIL,
  workspace: workspaceFixture(),
  conversation: conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
} as const;
it('requires a matching ready Agent and current access even with complete history', () => {
  for (const state of [
    { status: 'idle' },
    { status: 'loading' },
    { status: 'error', message: 'Denied' },
  ] as AgentResourceState[])
    expect(controlBaselineReady(state, 'agt_1')).toBe(false);
  expect(controlBaselineReady(ready, undefined)).toBe(false);
  expect(controlBaselineReady(ready, 'agt_other')).toBe(false);
  expect(controlBaselineReady(ready, 'agt_1', true)).toBe(false);
  expect(controlBaselineReady(ready, 'agt_1')).toBe(true);
});
it('keeps legacy history gates and requires a v2 cursor, including an empty authorized head', async () => {
  for (const incomplete of [{ historyLoading: true }, { historyUnavailable: true }])
    expect(controlBaselineReady({ ...ready, ...incomplete }, 'agt_1')).toBe(false);
  const h = harness();
  const v2 = await h.client.agentTransport!.open('agt_1');
  expect(v2).toBeDefined();
  expect(controlBaselineReady({ ...ready, v2 }, 'agt_1')).toBe(false);
  for (const incomplete of [{ historyLoading: true }, { historyUnavailable: true }, {}])
    expect(
      controlBaselineReady(
        {
          ...ready,
          ...incomplete,
          v2,
          conversation: { ...ready.conversation, agentCursor: 'ag1.authorized.0' },
        },
        'agt_1',
      ),
    ).toBe(true);
});
