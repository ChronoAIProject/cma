import { expect, it, vi } from 'vitest';
import {
  ApiError,
  createClient,
  ProtocolError,
  ValidationError,
  type DocumentUsage,
  type DocumentUsageQuota,
} from '../src/index.js';
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

it('reads typed document quotas for the authenticated owner and forwards operation options', async () => {
  const quota: DocumentUsageQuota = { used: 2, limit: 100 };
  const usage: DocumentUsage = {
    enforcement: 'enforced',
    documents: quota,
    bytes: { used: 1024, limit: 1073741824 },
    objects: { used: 3, limit: 32768 },
    pending_deletions: null,
  };
  const fetch = vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(usage)));
  const client = createClient({
    baseUrl: 'https://cma.example.test',
    credential: () => 'usage-fixture',
    fetch,
  });
  expect(await client.usage.documents({ requestId: 'documents-read' })).toEqual(usage);
  const [url, init] = fetch.mock.calls[0]!;
  expect(url).toBe('https://cma.example.test/api/v1/usage/documents');
  expect(init?.method).toBe('GET');
  const headers = new Headers(init?.headers);
  expect(headers.get('authorization')).toBe('Bearer usage-fixture');
  expect(headers.get('x-request-id')).toBe('documents-read');
});

it('preserves null document quota counters while enforcement is disabled', async () => {
  const test = transport();
  const usage: DocumentUsage = {
    enforcement: 'not_enforced',
    documents: null,
    bytes: null,
    objects: null,
    pending_deletions: null,
  };
  test.answer(usage);
  expect(await test.client.usage.documents()).toEqual(usage);
});

it('rejects malformed document quota counters and unknown enforcement states', async () => {
  const test = transport();
  const usage = {
    enforcement: 'enforced',
    documents: { used: 2, limit: 100 },
    bytes: { used: 1024, limit: 1073741824 },
    objects: { used: 3, limit: 32768 },
    pending_deletions: null,
  };
  for (const invalid of [
    { ...usage, enforcement: 'unknown' },
    { ...usage, documents: undefined },
    { ...usage, documents: { used: -1, limit: 100 } },
    { ...usage, bytes: { used: 1.5, limit: 1073741824 } },
    { ...usage, objects: { used: 3, limit: Number.MAX_SAFE_INTEGER + 1 } },
  ]) {
    test.answer(invalid);
    await expect(test.client.usage.documents()).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('preserves named authorization failures from document usage reads', async () => {
  const test = transport();
  test.answer({ status: 401, code: 'unauthenticated', detail: 'Authentication required.' }, 401);
  await expect(test.client.usage.documents()).rejects.toMatchObject({
    status: 401,
    code: 'unauthenticated',
  } satisfies Partial<ApiError>);
});
