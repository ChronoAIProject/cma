import { expect, it, vi } from 'vitest';
import {
  agent,
  page,
  item,
  json,
  harness,
  frame,
  sse,
  durable,
  cursor,
} from './agent-events-v2-fixtures.js';
import { ProtocolError } from '../src/index.js';
it('opts into exact message identities on snapshots and streams while accepting old responses', async () => {
  const t = harness();
  const value = page(1);
  Object.assign(value.items[0]!, {
    kind: { type: 'message', role: 'user', content: 'same' },
    message_command_id: 'cmd_steer',
  });
  t.fetch.mockResolvedValueOnce(json(value));
  expect(
    (await t.api.snapshot(agent, { messageBindings: true })).items[0]?.message_command_id,
  ).toBe('cmd_steer');
  expect(new Headers(t.fetch.mock.calls[0]![1]?.headers).get('Prefer')).toBe(
    'cma-message-bindings',
  );
  const event = {
    ...durable(2, { type: 'message', role: 'user', content: 'same' }),
    message_command_id: 'cmd_second',
  };
  t.fetch.mockResolvedValueOnce(sse(frame('cma.agent.event', event, cursor(2))));
  const received = vi.fn();
  await t.api.events(agent, {
    afterCursor: cursor(1),
    follow: false,
    messageBindings: true,
    onEvent: received,
  });
  expect(new Headers(t.fetch.mock.calls[1]![1]?.headers).get('Prefer')).toBe(
    'cma-message-bindings',
  );
  expect(received.mock.calls[0]?.[0].data.message_command_id).toBe('cmd_second');
  t.fetch.mockResolvedValueOnce(json(page(1)));
  expect(
    (await t.api.snapshot(agent, { messageBindings: true })).items[0]?.message_command_id,
  ).toBeUndefined();
});
it.each([
  null,
  { type: 'message', role: 'assistant', content: 'reply' },
  { type: 'turn_started', turn_sequence: 1 },
])('rejects a message command attached to non-user content %j', async (kind) => {
  const t = harness();
  const value = { ...page(1), items: [{ ...item(1, kind), message_command_id: 'cmd_wrong' }] };
  t.fetch.mockResolvedValueOnce(json(value));
  await expect(t.api.snapshot(agent, { messageBindings: true })).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
