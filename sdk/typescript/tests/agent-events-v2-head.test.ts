import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { agent, cursor, incarnation, item, json, harness } from './agent-events-v2-fixtures.js';

const head = (active = true) => ({
  object: 'cma.agent.event_head',
  schema_version: 1,
  agent_id: agent,
  incarnation,
  through_cursor: cursor(8),
  active_turn: active ? item(3, { type: 'turn_started', turn_sequence: 3 }) : null,
});

it('reads one authenticated current baseline without fetching history', async () => {
  const t = harness();
  const value = head();
  value.active_turn!.response_binding = {
    command_id: 'cmd_active',
    response_id: 'resp_active',
    turn_sequence: 3,
  };
  t.fetch.mockResolvedValueOnce(json(value));
  const controller = new AbortController();
  const result = await t.api.head(agent, { signal: controller.signal });
  expect(result).toEqual(value);
  expect(t.fetch).toHaveBeenCalledOnce();
  const [url, options] = t.fetch.mock.calls[0]!;
  expect(String(url)).toBe(`https://cma.example/gateway/api/v2/agents/${agent}/events/head`);
  expect(options!.method).toBe('GET');
  expect(options!.body).toBeUndefined();
  expect(new Headers(options!.headers).get('authorization')).toBe('Bearer secret');
  expect(options!.signal!.aborted).toBe(false);
});

it.each([0, 8])('retains idle head %s without inventing an active turn', async (sequence) => {
  const t = harness();
  const value = { ...head(false), through_cursor: cursor(sequence) };
  t.fetch.mockResolvedValueOnce(json(value));
  expect(await t.api.head(agent)).toEqual(value);
});

const mutations: [string, (value: ReturnType<typeof head>) => void][] = [
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
    'unknown field',
    (value) => {
      Object.assign(value, { private: true });
    },
  ],
  [
    'wrong version',
    (value) => {
      value.schema_version = 2;
    },
  ],
  [
    'future turn',
    (value) => {
      value.through_cursor = cursor(2);
    },
  ],
  [
    'non-start row',
    (value) => {
      value.active_turn!.kind = { type: 'turn_completed', turn_sequence: 3 };
    },
  ],
  [
    'hidden row',
    (value) => {
      value.active_turn!.kind = null;
    },
  ],
  [
    'wrong turn sequence',
    (value) => {
      value.active_turn!.kind = { type: 'turn_started', turn_sequence: 2 };
    },
  ],
  [
    'cursor disagreement',
    (value) => {
      value.active_turn!.cursor = cursor(4);
    },
  ],
  [
    'foreign turn incarnation',
    (value) => {
      value.active_turn!.cursor = `ag1.${'b'.repeat(64)}.3`;
    },
  ],
  [
    'wrong response binding',
    (value) => {
      value.active_turn!.response_binding = {
        command_id: 'cmd',
        response_id: 'resp',
        turn_sequence: 2,
      };
    },
  ],
  [
    'private metadata',
    (value) => {
      Object.assign(value.active_turn!.kind as object, { origin_key: 'private' });
    },
  ],
  [
    'unsafe sequence',
    (value) => {
      value.active_turn!.sequence = Number.MAX_SAFE_INTEGER + 1;
    },
  ],
  [
    'missing active turn',
    (value) => {
      delete (value as Partial<typeof value>).active_turn;
    },
  ],
];
it.each(mutations)('rejects head with %s', async (_, mutate) => {
  const t = harness();
  const value = head();
  mutate(value);
  t.fetch.mockResolvedValueOnce(json(value));
  await expect(t.api.head(agent)).rejects.toBeInstanceOf(ProtocolError);
});

it('validates the Agent identity before IO', async () => {
  const t = harness();
  await expect(t.api.head('')).rejects.toBeInstanceOf(ValidationError);
  expect(t.fetch).not.toHaveBeenCalled();
});

it('cancels a pending head read using the caller signal', async () => {
  const t = harness();
  let started!: () => void;
  const ready = new Promise<void>((resolve) => {
    started = resolve;
  });
  t.fetch.mockImplementationOnce(async (_url, options) => {
    started();
    await new Promise<void>((_resolve, reject) => {
      options!.signal!.addEventListener('abort', () => reject(options!.signal!.reason));
    });
    return json(head());
  });
  const controller = new AbortController();
  const pending = t.api.head(agent, { signal: controller.signal });
  const rejected = expect(pending).rejects.toBeDefined();
  await ready;
  controller.abort();
  await rejected;
  expect(t.fetch).toHaveBeenCalledOnce();
});
