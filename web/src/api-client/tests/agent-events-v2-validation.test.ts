// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { ProtocolError, TimeoutError, TransportError, ValidationError } from '../index.js';
import {
  agent,
  incarnation,
  cursor,
  pageCursor,
  page,
  durable,
  live,
  gap,
  json,
  frame,
  sse,
  harness,
  eligibility,
} from './agent-events-v2-fixtures.js';

const snapshotMutations: [string, (value: ReturnType<typeof page>) => void][] = [
  ['unknown top-level field', (value) => Object.assign(value, { runtime_locator: 'private' })],
  [
    'wrong schema',
    (value) => {
      value.schema_version = 2;
    },
  ],
  [
    'wrong Agent',
    (value) => {
      value.agent_id = 'other';
    },
  ],
  [
    'wrong incarnation',
    (value) => {
      value.incarnation = 'b'.repeat(64);
    },
  ],
  [
    'unsafe chronology',
    (value) => {
      value.items[0]!.sequence = Number.MAX_SAFE_INTEGER + 1;
    },
  ],
  [
    'skipped row',
    (value) => {
      value.items.shift();
    },
  ],
  [
    'duplicate row',
    (value) => {
      value.items[1] = value.items[0]!;
    },
  ],
  [
    'cursor disagreement',
    (value) => {
      value.items[0]!.cursor = cursor(2);
    },
  ],
  [
    'future turn',
    (value) => {
      value.items[0]!.kind = { type: 'turn_started', turn_sequence: 3 };
    },
  ],
  [
    'private kind',
    (value) => {
      value.items[0]!.kind = {
        type: 'conversation_opened',
        conversation_id: 'secret',
        runtime_generation: 1,
      };
    },
  ],
  [
    'unknown nested field',
    (value) => {
      value.items[0]!.kind = {
        type: 'message',
        role: 'assistant',
        content: 'safe',
        reasoning: 'private',
      };
    },
  ],
  [
    'missing nullable title',
    (value) => {
      value.items[0]!.kind = { type: 'tool_updated', call_id: 'call', status: 'completed' };
    },
  ],
  [
    'unknown nested plan field',
    (value) => {
      value.items[0]!.kind = {
        type: 'plan_set',
        entries: [{ title: 'step', status: 'pending', secret: true }],
      };
    },
  ],
  [
    'response binding on message',
    (value) => {
      value.items[0]!.response_binding = {
        command_id: 'cmd',
        response_id: 'resp',
        turn_sequence: 1,
      };
    },
  ],
  [
    'early page termination',
    (value) => {
      value.items.pop();
    },
  ],
  [
    'empty partial page',
    (value) => {
      value.items = [];
      value.next_page_cursor = pageCursor(2, 0);
    },
  ],
  [
    'invented continuation',
    (value) => {
      value.next_page_cursor = pageCursor(3, 2);
    },
  ],
  [
    'missing nullable kind',
    (value) => {
      delete (value.items[0] as Partial<(typeof value.items)[0]>).kind;
    },
  ],
  [
    'missing binding',
    (value) => {
      delete (value.items[0] as Partial<(typeof value.items)[0]>).response_binding;
    },
  ],
];
it.each(snapshotMutations)('rejects snapshot %s', async (_, mutate) => {
  const value = page();
  mutate(value);
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(value));
  await expect(t.api.snapshot(agent)).rejects.toBeInstanceOf(ProtocolError);
});
it('rejects a new snapshot watermark on a retained continuation', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(page(5, 2, 2)));
  await expect(
    t.api.snapshot(agent, { pageCursor: pageCursor(4, 2), limit: 2 }),
  ).rejects.toBeInstanceOf(ProtocolError);
});
it('rejects a page containing more rows than requested', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(page()));
  await expect(t.api.snapshot(agent, { limit: 1 })).rejects.toBeInstanceOf(ProtocolError);
});
it.each([
  ['missing durable ID', frame('cma.agent.event', durable(1))],
  ['wrong durable ID', frame('cma.agent.event', durable(1), cursor(2))],
  ['skipped sequence', frame('cma.agent.event', durable(2), cursor(2))],
  ['changed Agent', frame('cma.agent.event', { ...durable(1), agent_id: 'other' }, cursor(1))],
  [
    'changed incarnation',
    frame('cma.agent.event', { ...durable(1), incarnation: 'b'.repeat(64) }, cursor(1)),
  ],
  ['unknown event', frame('cma.agent.ready', {})],
  ['invalid JSON', 'event: cma.agent.event\ndata: {\n\n'],
  [
    'live ID',
    frame('cma.agent.live', { ...live(), turn_sequence: 1, after_sequence: 1 }, cursor(1)),
  ],
  ['live future anchor', frame('cma.agent.live', live())],
  ['live unknown kind', frame('cma.agent.live', { ...live(), kind: 'private_reasoning' })],
  ['gap ID', frame('cma.agent.live_gap', { ...gap(), turn_sequence: null }, '')],
  [
    'invented reconnect count',
    frame('cma.agent.live_gap', { ...gap(), turn_sequence: null, frames: 7 }),
  ],
  ['future gap turn', frame('cma.agent.live_gap', gap())],
  [
    'error ID',
    frame(
      'cma.agent.error',
      { code: 'agent_history_unavailable', message: 'Missing history' },
      cursor(0),
    ),
  ],
])('rejects %s before consumer acknowledgement', async (_, body) => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(sse(body));
  const onEvent = vi.fn();
  const onCursor = vi.fn();
  await expect(t.api.events(agent, { onEvent, onCursor })).rejects.toBeInstanceOf(ProtocolError);
  expect(onEvent).not.toHaveBeenCalled();
  expect(onCursor).not.toHaveBeenCalled();
});
it.each(['delimiter', 'UTF-8'] as const)(
  'reports EOF inside a partial %s as a retryable interruption without acknowledging it',
  async (cut) => {
    const t = harness();
    const complete = frame('cma.agent.event', durable(1), cursor(1));
    const partial = frame(
      'cma.agent.event',
      durable(2, { type: 'message', role: 'assistant', content: '🙂' }),
      cursor(2),
    );
    const body = new TextEncoder().encode(complete + partial);
    const end = cut === 'delimiter' ? body.length - 1 : body.indexOf(0xf0) + 2;
    t.fetch.mockResolvedValueOnce(sse(body.slice(0, end), 19));
    const onEvent = vi.fn();
    const onCursor = vi.fn();
    await expect(t.api.events(agent, { onEvent, onCursor })).rejects.toBeInstanceOf(TransportError);
    expect(onEvent).toHaveBeenCalledOnce();
    expect(onCursor.mock.calls).toEqual([[cursor(1)]]);
  },
);
it('resumes the durable cursor after a severed UTF-8 SSE comment', async () => {
  const t = harness();
  const complete = frame('cma.agent.event', durable(1), cursor(1));
  const interrupted = new TextEncoder().encode(complete + ': keepalive 🙂');
  t.fetch.mockResolvedValueOnce(sse(interrupted.slice(0, -2), 19));
  t.fetch.mockResolvedValueOnce(
    sse(': keepalive 🙂\n\n' + frame('cma.agent.event', durable(2), cursor(2)), 1),
  );
  const onEvent = vi.fn();
  const onCursor = vi.fn();
  await expect(t.api.events(agent, { onEvent, onCursor })).rejects.toBeInstanceOf(TransportError);
  expect(onEvent).toHaveBeenCalledOnce();
  expect(onCursor.mock.calls).toEqual([[cursor(1)]]);
  await t.api.events(agent, { afterCursor: cursor(1), follow: false, onEvent, onCursor });
  expect(onEvent.mock.calls.map(([event]) => event.data.sequence)).toEqual([1, 2]);
  expect(onCursor.mock.calls).toEqual([[cursor(1)], [cursor(2)]]);
  expect(new URL(String(t.fetch.mock.calls[1]![0])).searchParams.get('after_cursor')).toBe(
    cursor(1),
  );
});
it('rejects malformed UTF-8 inside a comment during streaming', async () => {
  const t = harness();
  const bytes = new TextEncoder().encode(': malformed ');
  t.fetch.mockResolvedValueOnce(sse(Uint8Array.from([...bytes, 0xf0, 0x28, 0x0a]), 1));
  await expect(t.api.events(agent, { onEvent: vi.fn() })).rejects.toBeInstanceOf(ProtocolError);
});
it('rejects invalid UTF-8 and stops observation', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(sse(new Uint8Array([0xff])));
  await expect(t.api.events(agent, { onEvent: vi.fn() })).rejects.toBeInstanceOf(ProtocolError);
});
it('bounds raw incomplete UTF-8 frames across many chunks', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(sse('data: ' + '🙂'.repeat(1_048_900), 32_768));
  await expect(t.api.events(agent, { onEvent: vi.fn() })).rejects.toBeInstanceOf(ProtocolError);
});
it('bounds complete durable frame bytes and pending transport chunks', async () => {
  for (const length of [4 * 1024 * 1024, 9 * 1024 * 1024]) {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      sse(
        frame(
          'cma.agent.event',
          durable(1, { type: 'message', role: 'assistant', content: 'x'.repeat(length) }),
          cursor(1),
        ),
      ),
    );
    await expect(t.api.events(agent, { onEvent: vi.fn() })).rejects.toBeInstanceOf(ProtocolError);
  }
});
it('accepts a durable frame larger than the older stream one-MiB bound', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(
    sse(
      frame(
        'cma.agent.event',
        durable(1, { type: 'message', role: 'assistant', content: 'x'.repeat(2 * 1024 * 1024) }),
        cursor(1),
      ),
      65_536,
    ),
  );
  const onCursor = vi.fn();
  await t.api.events(agent, { onEvent: vi.fn(), onCursor });
  expect(onCursor).toHaveBeenCalledWith(cursor(1));
});
it('bounds snapshot bytes including whitespace and content-length', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(
    new Response(' '.repeat(4 * 1024 * 1024) + JSON.stringify(page()), {
      headers: { 'content-type': 'application/json' },
    }),
  );
  await expect(t.api.snapshot(agent)).rejects.toBeInstanceOf(ProtocolError);
  t.fetch.mockResolvedValueOnce(
    new Response('{}', {
      headers: { 'content-type': 'application/json', 'content-length': '4194305' },
    }),
  );
  await expect(t.api.snapshot(agent)).rejects.toBeInstanceOf(ProtocolError);
});
it('requires exact JSON media and rejects a malformed or unavailable eligibility result', async () => {
  for (const response of [
    new Response('{}'),
    new Response('{', { headers: { 'content-type': 'application/json' } }),
    json({ ...eligibility(), schema_version: 9 }),
    json({ ...eligibility(), controls: '/api/v1/interrupt' }),
  ]) {
    const t = harness();
    t.fetch.mockResolvedValueOnce(response);
    await expect(t.api.readTransportEligibility()).rejects.toBeInstanceOf(ProtocolError);
  }
});
it('keeps the deadline active while reading a successful snapshot body', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(
    new Response(new ReadableStream(), { headers: { 'content-type': 'application/json' } }),
  );
  await expect(t.api.snapshot(agent, { timeoutMs: 10 })).rejects.toBeInstanceOf(TimeoutError);
});
it.each(['', 'bad', `ags1.${incarnation}.2.2`, cursor(1)])(
  'rejects invalid page cursor before IO',
  async (value) => {
    const t = harness();
    await expect(t.api.snapshot(agent, { pageCursor: value })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(t.fetch).not.toHaveBeenCalled();
  },
);
