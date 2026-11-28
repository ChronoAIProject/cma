import { expect, it } from 'vitest';
import {
  parkedRuntimeUpdateFixture,
  parkedRuntimeResultFixture,
  workspaceFixture,
} from '../test/hierarchy';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
import { sandboxDeletionReason } from './deletionEligibility';
import { taskPresentation } from '../shell/taskPresentation';
import { sandboxNavigation } from '../shell/navigationStatuses';
import { sandboxStatusPresentation } from './presentation';
import { sandboxChildrenAvailable, sandboxRefreshDelay } from './lifecycle';

it.each(['image_rotation', 'revive'] as const)(
  'uses the list result for parked %s without detail-only runtime metadata',
  (purpose) => {
    const base = parkedRuntimeUpdateFixture();
    const sandbox = {
      ...base,
      status: purpose === 'revive' ? ('reviving' as const) : ('rotating' as const),
      runtimeUpdate: undefined,
      lifecycle: {
        ...base.lifecycle!,
        operation: { ...base.lifecycle!.operation!, purpose },
        result: {
          state: 'failed' as const,
          code: 'runtime_update_parked',
          continuing: false,
          startedAt: base.lifecycle!.operation!.startedAt,
          actions: ['observe' as const],
        },
      },
    };
    expect(sandboxStatusPresentation(sandbox)).toMatchObject({
      status: sandbox.status,
      group: 'needs_attention',
      label: 'Runtime update needs attention',
      tone: 'error',
    });
    expect(sandboxChildrenAvailable(sandbox)).toBe(false);
    expect(sandboxRefreshDelay(sandbox)).toBeGreaterThanOrEqual(5000);
  },
);

it('recognizes old rotation details conservatively and rejects completed or unrelated failures', () => {
  const base = parkedRuntimeUpdateFixture();
  expect(sandboxHasParkedRuntimeUpdate(base)).toBe(true);
  for (const sandbox of [
    { ...base, status: 'active' as const },
    { ...base, lifecycle: { ...base.lifecycle!, operation: undefined } },
    {
      ...base,
      lifecycle: {
        ...base.lifecycle!,
        operation: { ...base.lifecycle!.operation!, purpose: 'revive' as const },
      },
    },
    { ...base, runtimeUpdate: { ...base.runtimeUpdate!, completedAt: base.updatedAt } },
    { ...base, runtimeUpdate: { ...base.runtimeUpdate!, startedAt: '2000-01-01T00:00:00Z' } },
  ])
    expect(sandboxHasParkedRuntimeUpdate(sandbox)).toBe(false);
});

it('ignores stale admission results and clears attention on an accepted running receipt', () => {
  const base = parkedRuntimeResultFixture();
  for (const result of [
    { ...base.lifecycle!.result!, startedAt: undefined },
    { ...base.lifecycle!.result!, startedAt: '2000-01-01T00:00:00Z' },
    { ...base.lifecycle!.result!, continuing: true },
    { ...base.lifecycle!.result!, state: 'in_progress' as const },
    { ...base.lifecycle!.result!, code: 'lifecycle_failed' },
  ])
    expect(
      sandboxHasParkedRuntimeUpdate({ ...base, lifecycle: { ...base.lifecycle!, result } }),
    ).toBe(false);
  const resumed = {
    ...base,
    runtimeUpdate: { ...parkedRuntimeUpdateFixture().runtimeUpdate!, state: 'updating' as const },
  };
  expect(sandboxHasParkedRuntimeUpdate(resumed)).toBe(false);
  expect(sandboxRefreshDelay(resumed)).toBe(1500);
  expect(sandboxRefreshDelay(base, true)).toBe(5000);
  expect(sandboxStatusPresentation(base, true).group).toBe('deleting');
  for (const status of ['retired', 'retiring', 'failed'] as const) {
    expect(sandboxHasParkedRuntimeUpdate({ ...base, status })).toBe(false);
  }
});

it('groups a fresh inventory result without fetching details and preserves task history', () => {
  const base = parkedRuntimeResultFixture('revive');
  const groups = sandboxNavigation([base], (value) => value, 'all').groups;
  expect(groups.find((group) => group.id === 'needs_attention')?.items).toEqual([base]);
  expect(groups.find((group) => group.id === 'changing')?.items).toEqual([]);
  expect(sandboxNavigation([base], (value) => value, 'failed').visible).toEqual([base]);
  for (const [taskStatus, presentedStatus, label] of [
    ['completed', 'completed', 'Turn ended'],
    ['stopped', 'interrupted', 'Response interrupted'],
    ['failed', 'failed', 'Failed'],
    ['working', 'needs_attention', 'Needs attention'],
  ] as const) {
    const agent = { ...workspaceFixture().agents[0]!, taskStatus };
    const task = taskPresentation(agent, base, 'fresh');
    expect(task?.status).toBe(presentedStatus);
    expect(task?.label).toBe(label);
    expect(task?.showRunningTime).toBe(false);
    expect(task?.parentNotice).toBe('Runtime update failed');
  }
  const stoppedAgent = {
    ...workspaceFixture().agents[0]!,
    status: 'stopped',
    taskStatus: 'stopped',
  } as const;
  expect(taskPresentation(stoppedAgent, base, 'fresh')).toMatchObject({
    status: 'stopped',
    label: 'Stopped',
    parentNotice: 'Runtime update failed',
  });
});

it('does not grant a parked revival deletion without the server retire action', () => {
  const base = parkedRuntimeResultFixture('revive');
  expect(sandboxDeletionReason(base)).toContain('Wait');
  const allowed = {
    ...base,
    lifecycle: {
      ...base.lifecycle!,
      result: { ...base.lifecycle!.result!, actions: ['observe', 'retire'] as const },
    },
  };
  expect(sandboxDeletionReason(allowed)).toBeUndefined();
  expect(sandboxDeletionReason(allowed, { stale: true })).toContain('Checking');
  expect(sandboxDeletionReason(allowed, { busy: true })).toContain('Wait');
});

it('allows Delete for a stalled rotation with current server retirement authority', () => {
  const base = parkedRuntimeResultFixture();
  const stalled = {
    ...base,
    lifecycle: {
      ...base.lifecycle!,
      result: {
        ...base.lifecycle!.result!,
        state: 'recoverable_pending' as const,
        continuing: true,
        code: 'runtime_update_stalled',
        actions: ['observe', 'retire'] as const,
      },
    },
  };
  expect(sandboxDeletionReason(stalled, { lifecycleBusy: true })).toBeUndefined();
  expect(sandboxDeletionReason(stalled, { busy: true })).toContain('Wait');
  expect(sandboxDeletionReason(stalled, { stale: true })).toContain('Checking');
  for (const result of [
    { ...stalled.lifecycle.result, actions: ['observe'] as const },
    { ...stalled.lifecycle.result, startedAt: '2000-01-01T00:00:00Z' },
    { ...stalled.lifecycle.result, code: 'lifecycle_wait_exceeded' },
  ]) {
    expect(
      sandboxDeletionReason({ ...stalled, lifecycle: { ...stalled.lifecycle, result } }),
    ).toContain('Wait');
  }
});
