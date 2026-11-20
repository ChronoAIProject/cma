// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../index.js';
import { transport } from './resource-fixtures.js';

const totals = {
  uptime_seconds: 3600,
  cpu_core_hours: 0.5,
  memory_gib_hours: 2,
  disk_gib_hours: 20,
  cpu_millis_seconds: 1800000,
  memory_mib_seconds: 7372800,
  disk_gib_seconds: 72000,
};
const row = {
  sandbox_id: 'sandbox-1',
  owner: 'owner-1',
  name: 'Research',
  status: 'paused',
  allocation: { running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 20 },
  created_at: 1000,
  tracking_started_at: 1000,
  as_of: 4600,
  complete_lifecycle: true,
  totals,
};
it('uses the independent usage route group for owner, sandbox and admin reads', async () => {
  const test = transport();
  test.answer(row);
  expect(await test.client.usage.sandbox('sandbox-1')).toEqual(row);
  expect(test.calls.at(-1)?.url).toBe('https://cma.example.test/api/v1/usage/sandboxes/sandbox-1');
  test.answer({ items: [row], next: 'sandbox-1', as_of: 4600 });
  await test.client.usage.adminSandboxes({ owner: 'owner-1', cursor: 'previous', limit: 25 });
  expect(test.calls.at(-1)?.url).toBe(
    'https://cma.example.test/api/v1/admin/usage/sandboxes?limit=25&cursor=previous&owner=owner-1',
  );
  test.answer({ items: [], next: null });
  expect((await test.client.usage.events('sandbox-1')).items).toEqual([]);
});
it('rejects wrong sandbox identities, unsafe quantities and invalid paging', async () => {
  const test = transport();
  test.answer({ ...row, sandbox_id: 'another' });
  await expect(test.client.usage.sandbox('sandbox-1')).rejects.toBeInstanceOf(ProtocolError);
  test.answer({ ...row, totals: { ...totals, uptime_seconds: -1 } });
  await expect(test.client.usage.sandbox('sandbox-1')).rejects.toBeInstanceOf(ProtocolError);
  expect(() => test.client.usage.sandboxes({ limit: 201 })).toThrow(ValidationError);
});

it('passes service-issued continuations unchanged across all Usage page endpoints', async () => {
  const test = transport();
  const token = `u1.${'a'.repeat(384)}.${'b'.repeat(43)}`;
  for (const request of [
    (cursor?: string) => test.client.usage.sandboxes({ cursor }),
    (cursor?: string) => test.client.usage.adminSandboxes({ cursor, owner: 'owner-1' }),
    (cursor?: string) => test.client.usage.events('sandbox-1', { cursor }),
  ]) {
    test.answer({ items: [], next: token, as_of: 4600 });
    const first = await request();
    expect(first.next).toBe(token);
    test.answer({ items: [], next: null, as_of: 4600 });
    expect((await request(first.next ?? undefined)).next).toBeNull();
    expect(new URL(test.calls.at(-1)!.url).searchParams.get('cursor')).toBe(token);
  }
});

it('bounds Usage cursors in UTF-8 bytes while retaining raw continuation compatibility', async () => {
  const test = transport();
  for (const cursor of ['sandbox-1', 'a'.repeat(4096)]) {
    test.answer({ items: [], next: null, as_of: 4600 });
    await test.client.usage.sandboxes({ cursor });
    expect(new URL(test.calls.at(-1)!.url).searchParams.get('cursor')).toBe(cursor);
  }
  const before = test.calls.length;
  for (const cursor of ['', 'a'.repeat(4097), '界'.repeat(1366)]) {
    expect(() => test.client.usage.sandboxes({ cursor })).toThrow(ValidationError);
    expect(() => test.client.usage.adminSandboxes({ cursor })).toThrow(ValidationError);
    expect(() => test.client.usage.events('sandbox-1', { cursor })).toThrow(ValidationError);
  }
  expect(test.calls).toHaveLength(before);
});
