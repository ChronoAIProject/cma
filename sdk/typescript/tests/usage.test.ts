import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
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
