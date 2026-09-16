import { readFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, ValidationError } from '../src/index.js';
const corpus = JSON.parse(
  readFileSync(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-partitions.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
const detail = () =>
  structuredClone(
    corpus.cases.find((c: { id: string }) => c.id === 'partition-owner-detail').response.body,
  );
const bridge = detail().partition.bridge_id as string;
const allocation = detail().partition.id as string;
const response = (body: unknown) =>
  new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } });
const cursor = (scope: string, id: string) =>
  Buffer.from(JSON.stringify(['cma.bridge.partitions.v1', scope, id])).toString('base64url');
const command = () => ({
  command_id: 'cmd_1',
  response_id: 'resp_1',
  config_revision: 1,
  agent_order: 1,
  status: 'waiting_action',
  waiting_for: 'worker',
  turn_sequence: null,
  delivery: null,
  admitted_at: '2026-09-15T06:00:00Z',
});

it('reads exact partition generations using GET and preserves canonical scoped continuation', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'fixture',
    fetch,
  });
  const value = detail();
  value.activity.pending = [command()];
  value.activity.latest = command();
  fetch.mockResolvedValueOnce(response(value));
  expect(await client.bridges.readPartition(bridge, allocation)).toEqual(value);
  expect(fetch.mock.calls[0]![1].method).toBe('GET');
  expect(fetch.mock.calls[0]![1].body).toBeUndefined();
  const page = { items: [value.partition], next_cursor: cursor(bridge, allocation) };
  fetch.mockResolvedValueOnce(response(page));
  expect(await client.bridges.listPartitions(bridge, { limit: 1 })).toEqual(page);
  fetch.mockResolvedValueOnce(response({ items: [], next_cursor: null }));
  await client.bridges.listPartitions(bridge, { cursor: page.next_cursor, limit: 1 });
  expect(new URL(fetch.mock.calls[2]![0]).searchParams.get('cursor')).toBe(page.next_cursor);
  for (const bad of [
    cursor('another', allocation),
    page.next_cursor + '=',
    Buffer.from(JSON.stringify(['cma.bridge.partitions.v1', bridge, allocation], null, 1)).toString(
      'base64url',
    ),
  ]) {
    expect(() => client.bridges.listPartitions(bridge, { cursor: bad })).toThrow(ValidationError);
  }
  expect(() => client.bridges.listPartitions(bridge, { limit: 26 })).toThrow(ValidationError);
  expect(fetch).toHaveBeenCalledTimes(3);
});

it('refuses public secret fields, inconsistent redaction and uncorrelated pending status', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'fixture',
    fetch,
  });
  const badValues = [
    { ...detail(), activity: { status: 'redacted' } },
    { ...detail(), partition: { ...detail().partition, generation: 0 } },
    {
      ...detail(),
      partition: {
        ...detail().partition,
        source: { ...detail().partition.source, text: 'secret' },
      },
    },
    { ...detail(), activity: { ...detail().activity, pending: [command()], latest: null } },
    {
      ...detail(),
      activity: { ...detail().activity, pending: [command(), command()], latest: command() },
    },
    {
      ...detail(),
      activity: {
        ...detail().activity,
        pending: [{ ...command(), turn_sequence: 2 }],
        latest: command(),
      },
    },
  ];
  for (const value of badValues) {
    fetch.mockResolvedValueOnce(response(value));
    await expect(client.bridges.readPartition(bridge, allocation)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
  const unavailable = detail();
  unavailable.partition.target = { status: 'reserved', ids: unavailable.partition.target.ids };
  unavailable.partition.source = { status: 'unavailable' };
  unavailable.activity = { status: 'unavailable' };
  fetch.mockResolvedValueOnce(response(unavailable));
  expect(await client.bridges.readPartition(bridge, allocation)).toEqual(unavailable);
});
