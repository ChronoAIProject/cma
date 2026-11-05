import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { agentClient } from '../test/agent';
import {
  conversationFromCurrentState,
  conversationFromWindow,
  withCurrentState,
} from './historyWindow';
import { loadAgent } from './loadAgent';
import { foldV2Item } from './transport/projection';
import { ConversationHydration } from './transport/hydration';
import { at, cursor, incarnation, json } from './transport/controller.fixture';

import { historyItem as item, historyPage, statePage } from './historyWindow.fixture';

it('keeps original turn identity and never binds a mid-turn owner message to its first command', () => {
  let state = conversationFromWindow({
    ...historyPage(),
    items: [
      item(901, { type: 'message', role: 'user', content: 'A later input' }),
      item(902, { type: 'tool_updated', call_id: 'tool-1', status: 'completed' }),
    ],
  });
  expect(state.activeRunId).toBe('1');
  expect(state.responseBinding?.commandId).toBe('cmd_1');
  expect(state.messages[0]?.commandId).toBeUndefined();
  expect(state.toolCalls).toEqual([
    expect.objectContaining({
      id: 'tool-1',
      title: 'Continued tool activity',
      status: 'completed',
    }),
  ]);
  state = foldV2Item(state, {
    ...item(903, { type: 'message', role: 'user', content: 'Steer' }),
    message_command_id: 'cmd_steer',
  });
  expect(state.messages.at(-1)?.commandId).toBe('cmd_steer');
});

it('preserves current approvals and commands whose source is outside the display window', () => {
  const state = withCurrentState(conversationFromWindow(historyPage()), statePage());
  expect(state.messages.map((message) => message.sequence)).toEqual([901, 902]);
  expect(state.approvals).toEqual([
    expect.objectContaining({ id: 'approval-1', message: 'Original request' }),
  ]);
  expect(state.plans[0]?.runId).toBe('1');
  expect(state.agentCursor).toBe(cursor(902));
  expect(() => withCurrentState(state, statePage(903))).toThrow('identity');
});

it('loads one bounded window without reading the complete transcript or collecting pages', async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    return json(url.pathname.endsWith('/state') ? statePage() : historyPage());
  });
  const sdk = createClient({
    baseUrl: 'https://api.example.test',
    fetch,
    credential: async () => 'fixture',
  });
  const transcript = vi.fn();
  const events = vi.fn();
  const client = agentClient({
    agentHistory: sdk.agentEventsV2,
    readAgentTranscript: transcript,
    readAgentEvents: events,
  });
  const ready = await loadAgent(client, 'agt_1', new AbortController().signal);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(transcript).not.toHaveBeenCalled();
  expect(events).not.toHaveBeenCalled();
  expect(ready.conversation.approvals[0]?.id).toBe('approval-1');
  expect(ready.conversation.historyPreviousCursor).toBe(cursor(900));
  expect(ready.durableEvents).toEqual([]);
});

it('bounds a stalled hydration buffer without moving the durable cursor backwards', () => {
  const hydration = new ConversationHydration();
  const baseline = conversationFromCurrentState(statePage());
  hydration.reset(baseline);
  let state = baseline;
  for (let seq = 903; seq < 1903; seq++)
    state = hydration.applyV2(
      state,
      {
        type: 'cma.agent.event',
        data: {
          ...item(seq, { type: 'message', role: 'assistant', content: 'Recorded output' }),
          agent_id: 'agt_1',
          incarnation,
        },
      },
      'agt_1',
      true,
    );
  const merged = hydration.merge(
    withCurrentState(conversationFromWindow(historyPage()), statePage()),
    'agt_1',
    true,
  );
  expect(merged.cursor).toBe(1902);
  expect(merged.agentCursor).toBe(cursor(1902));
  expect(merged.messages.length).toBeLessThanOrEqual(200);
  expect(merged.approvals[0]?.id).toBe('approval-1');
  expect(merged.responseBinding?.commandId).toBe('cmd_1');
  expect(merged.activeRunStartedAt).toBe(at);
});

it('preserves a preceding loss event across a mid-turn window and live display eviction', () => {
  let state = conversationFromWindow({
    ...historyPage(),
    gap: item(8, { type: 'history_gap', frames: 2 }),
  });
  expect(state.incompleteRuns).toEqual(['1']);
  for (let sequence = 903; sequence <= 1200; sequence++)
    state = foldV2Item(
      state,
      item(sequence, { type: 'message', role: 'assistant', content: 'Output' }),
    );
  expect(state.recovery.some((notice) => notice.id.startsWith('history-gap:'))).toBe(false);
  state = foldV2Item(state, item(1201, { type: 'turn_completed', turn_sequence: 1 }));
  expect(state.turnOutcomes?.['1']).toBe('failed');
  expect(state.messages.length).toBeLessThanOrEqual(200);
});

it('restores loss status for an active turn independently of the visible window', () => {
  const page = statePage();
  const state = conversationFromCurrentState({
    ...page,
    context: [...page.context, item(8, { type: 'history_gap', frames: 2 })],
  });
  expect(state.incompleteRuns).toEqual(['1']);
  expect(
    foldV2Item(state, item(903, { type: 'turn_completed', turn_sequence: 1 })).turnOutcomes?.['1'],
  ).toBe('failed');
});
