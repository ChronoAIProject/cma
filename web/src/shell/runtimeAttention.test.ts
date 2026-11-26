import { expect, it } from 'vitest';
import { navigationFixture, parkedRuntimeUpdateFixture } from '../test/hierarchy';
import { runtimeAttention } from './runtimeAttention';
import type { NavigationSandbox } from './navigation';

const parked = (): NavigationSandbox => ({
  sandbox: parkedRuntimeUpdateFixture(),
  workspaces: navigationFixture()[0]!.workspaces,
});

it('counts unique failed environments including environments without Agents', () => {
  const first = parked();
  const empty = { sandbox: { ...first.sandbox, id: 'sbx_empty' }, workspaces: [] };
  const result = runtimeAttention({ status: 'ready', items: [first, first, empty] });
  expect(result).toEqual({
    freshness: 'fresh',
    failedSandboxes: [
      { sandboxId: 'sbx_1', name: 'Atlas', agentIds: ['agt_1'] },
      { sandboxId: 'sbx_empty', name: 'Atlas', agentIds: [] },
    ],
  });
});

it('keeps Agent failures and approvals independent from environment updates', () => {
  for (const status of ['failed', 'waiting_approval'] as const)
    expect(
      runtimeAttention({ status: 'ready', items: navigationFixture('active', status) }),
    ).toEqual({ freshness: 'fresh', failedSandboxes: [] });
});

it('retains last-known observations without asserting current failures after a refresh error', () => {
  expect(
    runtimeAttention({ status: 'ready', items: [parked()], refreshError: 'Synthetic outage' }),
  ).toMatchObject({ freshness: 'stale', failedSandboxes: [{ sandboxId: 'sbx_1' }] });
  for (const state of [
    { status: 'loading' },
    { status: 'error', message: 'Access denied' },
  ] as const)
    expect(runtimeAttention(state)).toEqual({ freshness: 'unknown', failedSandboxes: [] });
});

it('excludes accepted and completed deletion while keeping UNKNOWN distinct', () => {
  const entry = parked();
  const ready = { status: 'ready', items: [entry] } as const;
  for (const status of ['retiring', 'retired'] as const)
    expect(
      runtimeAttention({ ...ready, items: [{ ...entry, sandbox: { ...entry.sandbox, status } }] }),
    ).toEqual({ freshness: 'fresh', failedSandboxes: [] });
  expect(runtimeAttention(ready, { accepted: new Set(['sbx_1']), unknown: new Set() })).toEqual({
    freshness: 'fresh',
    failedSandboxes: [],
  });
  expect(runtimeAttention(ready, { accepted: new Set(), unknown: new Set(['sbx_1']) })).toEqual({
    freshness: 'stale',
    failedSandboxes: [],
  });
});
