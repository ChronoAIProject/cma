import { readFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, type BridgeUsage } from '../src/index.js';

const corpus = JSON.parse(
  readFileSync(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-usage.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
const usage = (): BridgeUsage => structuredClone(corpus.cases[0].response.body);
const response = (value: unknown) =>
  new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } });

it('reads retained usage with GET, including explicit redaction and bounded inventory', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'fixture',
    fetch,
  });
  for (const row of corpus.cases.slice(0, 3)) {
    fetch.mockResolvedValueOnce(response(row.response.body));
    expect(await client.bridges.usage(usage().bridge_id)).toEqual(row.response.body);
  }
  for (const call of fetch.mock.calls) {
    expect(call[1].method).toBe('GET');
    expect(call[1].body).toBeUndefined();
    expect(new URL(call[0]).pathname).toBe(`/api/v1/bridges/${usage().bridge_id}/usage`);
  }
});

it('rejects inconsistent totals, hidden fields, mismatched scope and false capacity reasons', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'fixture',
    fetch,
  });
  const value = usage();
  const badValues = [
    { ...value, bridge_id: 'another' },
    { ...value, allocations: null },
    { ...value, inventory_limit: 255 },
    { ...value, observed_at_ms: -1 },
    { ...value, credential: 'private' },
    { ...value, allocations: { ...value.allocations, current: 2 } },
    { ...value, allocations: { ...value.allocations, ready: 1 } },
    { ...value, target_resources: { status: 'redacted' } },
    {
      ...value,
      target_resources: { ...value.target_resources, agents: { retained: 1, active: 2 } },
    },
    { ...value, capacity_reasons: ['partition_limit_reached'] },
    { ...value, capacity_reasons: ['retired_sandbox_reservations_retained'] },
    {
      ...value,
      capacity_reasons: ['shared_parent_policy_applies', 'shared_parent_policy_applies'],
    },
  ];
  for (const body of badValues) {
    fetch.mockResolvedValueOnce(response(body));
    await expect(client.bridges.usage(value.bridge_id)).rejects.toBeInstanceOf(ProtocolError);
  }
});
