/** @vitest-environment node */
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { createRestWebClient } from '../api/rest';
import { NOW, WIRE_AGENT, WIRE_WORKSPACE_DETAIL } from '../test/rest';
import { loadAgent } from './loadAgent';

it.each([
  ['ready', 'pre_write_failed'],
  ['ready', 'write_confirmed'],
  ['paused', 'pre_write_failed'],
  ['paused', 'write_confirmed'],
])('opens a %s Agent with retained conversation after settings %s', async (status, delivery) => {
  const agentPath = '/api/v1/agents/agent-1';
  const events = [
    { type: 'turn_started', turn_sequence: 1 },
    { type: 'owner_message', content: 'Review the project.' },
    { type: 'agent_message', content: 'The review is complete.' },
    { type: 'turn_completed', turn_sequence: 1 },
    {
      type: 'operation_delivery_changed',
      operation_sequence: 5,
      operation: 'update_settings',
      state: 'dispatch_fenced',
    },
    {
      type: 'operation_delivery_changed',
      operation_sequence: 5,
      operation: 'update_settings',
      state: delivery,
    },
  ].map((kind, index) => ({ seq: index + 1, appended_at: NOW, kind }));
  const messages = [
    { sequence: 2, role: 'user', content: 'Review the project.', appended_at: NOW },
    { sequence: 3, role: 'assistant', content: 'The review is complete.', appended_at: NOW },
  ];
  const responses: Record<string, unknown> = {
    [agentPath]: {
      ...WIRE_AGENT,
      status,
      sandbox: { ...WIRE_AGENT.sandbox, status: status === 'paused' ? 'paused' : 'active' },
    },
    [agentPath + '/transcript']: {
      agent_id: 'agent-1',
      through_seq: 6,
      turns: [
        {
          turn_sequence: 1,
          started_at: NOW,
          owner_message: messages[0]!.content,
          agent_messages: [messages[1]!.content],
          messages,
          outcome: { type: 'completed', at: NOW },
        },
      ],
    },
    [agentPath + '/usage']: {
      agent_id: 'agent-1',
      through_seq: 6,
      input_tokens: 10,
      output_tokens: 20,
      context_used: 30,
    },
    [agentPath + '/events']: { items: events },
    '/api/v1/workspaces/workspace-1': WIRE_WORKSPACE_DETAIL,
  };
  const fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
    const path = new URL(String(url)).pathname;
    expect(init?.method).toBe('GET');
    expect(Object.hasOwn(responses, path)).toBe(true);
    return Promise.resolve(
      new Response(JSON.stringify(responses[path]), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
  });
  const client = createRestWebClient(createClient({ baseUrl: 'https://cma.example.test', fetch }));
  const ready = await loadAgent(client, 'agent-1', new AbortController().signal, true);
  expect(ready.detail.status).toBe(status);
  expect(ready.historyUnavailable).toBe(false);
  expect(ready.historyLoading).toBe(false);
  expect(ready.conversation.cursor).toBe(6);
  expect(ready.conversation.activeRunId).toBeUndefined();
  expect(ready.conversation.messages.map(({ role, content }) => ({ role, content }))).toEqual(
    messages.map(({ role, content }) => ({ role, content })),
  );
  expect(ready.durableEvents.slice(-2).map((event) => event.kind)).toEqual(
    events.slice(-2).map((event) => event.kind),
  );
  expect(fetch).toHaveBeenCalledTimes(5);
});
