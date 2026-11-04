import { expect, it } from 'vitest';
import type { SandboxUsageView } from '@chronoai/cma-sdk';
import { operatorClient } from '../operator/testFixtures';
import { loadUsageInventory, selectUsage, summarizeUsage } from './inventory';

const reading: SandboxUsageView = {
  sandbox_id: 'sbx-a',
  name: 'Alpha',
  owner: 'owner-a',
  status: 'paused',
  allocation: { running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 20 },
  created_at: 1,
  tracking_started_at: 9,
  as_of: 100,
  complete_lifecycle: true,
  totals: {
    uptime_seconds: 9,
    cpu_millis_seconds: 9,
    memory_mib_seconds: 9,
    disk_gib_seconds: 9,
    cpu_core_hours: 0.0000025,
    memory_gib_hours: 9 / 3_686_400,
    disk_gib_hours: 0.0025,
  },
};
const filters = { owner: '', status: '', sandbox: '' };

it('orders raw quantities and timestamps numerically even when formatted values would tie', () => {
  const other = {
    ...reading,
    sandbox_id: 'sbx-b',
    tracking_started_at: 10,
    totals: {
      ...reading.totals,
      uptime_seconds: 10,
      cpu_millis_seconds: 10,
      memory_mib_seconds: 10,
      disk_gib_seconds: 10,
    },
  };
  for (const key of [
    'uptime_seconds',
    'cpu_millis_seconds',
    'memory_mib_seconds',
    'disk_gib_seconds',
    'tracking_started_at',
  ] as const) {
    expect(
      selectUsage([other, reading], filters, { key, direction: 'ascending' }, new Map()).map(
        (row) => row.sandbox_id,
      ),
    ).toEqual(['sbx-a', 'sbx-b']);
    expect(
      selectUsage([reading, other], filters, { key, direction: 'descending' }, new Map()).map(
        (row) => row.sandbox_id,
      ),
    ).toEqual(['sbx-b', 'sbx-a']);
  }
});

it('sorts users by displayed names and breaks ties by Sandbox ID deterministically', () => {
  const rows = [
    reading,
    { ...reading, sandbox_id: 'sbx-b', owner: 'owner-b' },
    { ...reading, sandbox_id: 'sbx-c' },
  ];
  const names = new Map([
    ['owner-a', 'Zoe'],
    ['owner-b', 'Alex'],
  ]);
  expect(
    selectUsage(rows, filters, { key: 'owner', direction: 'ascending' }, names).map(
      (row) => row.sandbox_id,
    ),
  ).toEqual(['sbx-b', 'sbx-a', 'sbx-c']);
  expect(summarizeUsage(rows, 100).totals.cpu_millis_seconds).toBe(27);
  expect(summarizeUsage([], 100).tracking_started_at).toBeNull();
});

it('refuses repeated cursors and incomplete inventories without publishing partial rows', async () => {
  const api = operatorClient().usage;
  api.adminSandboxes
    .mockResolvedValueOnce({ items: [reading], next: 'stuck', as_of: 100 })
    .mockResolvedValueOnce({ items: [], next: 'stuck', as_of: 100 });
  await expect(loadUsageInventory(api, new AbortController().signal)).rejects.toThrow(
    'did not advance',
  );
});

it('does not follow a page that completes after cancellation', async () => {
  const api = operatorClient().usage;
  const controller = new AbortController();
  api.adminSandboxes.mockImplementation(async () => {
    controller.abort();
    return { items: [reading], next: 'next-page', as_of: 100 };
  });
  await expect(loadUsageInventory(api, controller.signal)).rejects.toThrow();
  expect(api.adminSandboxes).toHaveBeenCalledTimes(1);
});
