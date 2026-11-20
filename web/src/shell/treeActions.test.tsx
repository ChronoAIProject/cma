import { expect, it } from 'vitest';
import type { TreePermissions } from './Tree';
import { agentActions } from './treeActions';
import { sandboxActionUnavailableReason, sandboxChildrenAvailable } from '../sandbox/lifecycle';
import { sandboxFixture } from '../test/hierarchy';

const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

it.each([
  ['stopped', 'Agent is already stopped.'],
  ['failed', 'Agent has failed. There is no running Agent to stop.'],
  ['deleted', 'Agent has been deleted.'],
] as const)('explains %s before runtime availability', (status, reason) => {
  for (const runtimeAvailable of [false, true]) {
    const actions = agentActions(
      status,
      permissions,
      runtimeAvailable,
      'Resume the Sandbox to continue working.',
    );
    expect(actions.find((action) => action.id === 'stop_agent')).toMatchObject({
      disabled: true,
      reason,
    });
    expect(actions.find((action) => action.id === 'delete_agent')?.disabled).toBe(
      !runtimeAvailable || status === 'deleted',
    );
    expect(actions.find((action) => action.id === 'export_history')?.disabled).toBeUndefined();
  }
});

it.each(['starting', 'ready', 'working', 'waiting_approval', 'paused'] as const)(
  'preserves Stop eligibility for %s',
  (status) => {
    for (const runtimeAvailable of [false, true]) {
      expect(
        agentActions(status, permissions, runtimeAvailable).find(
          (action) => action.id === 'stop_agent',
        )?.disabled,
      ).toBe(!runtimeAvailable);
    }
  },
);

it('omits Stop when the viewer lacks permission even for a terminal Agent', () => {
  for (const status of ['ready', 'stopped', 'failed', 'deleted'] as const)
    expect(
      agentActions(status, { ...permissions, stopAgent: false }, false).find(
        (action) => action.id === 'stop_agent',
      ),
    ).toBeUndefined();
});

it.each([
  { sandbox: sandboxFixture('paused'), reason: /resume the Sandbox/ },
  { sandbox: sandboxFixture('reviving'), reason: /^Resuming\./ },
  { sandbox: sandboxFixture('failed'), reason: /unavailable.*Check its status/ },
  { sandbox: undefined, reason: /^Checking Sandbox status\.$/ },
  { sandbox: sandboxFixture('paused'), stale: true, reason: /unavailable.*Check its status/ },
  { sandbox: sandboxFixture(), busy: true, reason: /action is in progress/ },
  {
    sandbox: { ...sandboxFixture(), lifecycle: undefined },
    reason: /availability is not reported/,
  },
])(
  'explains trusted runtime context for a nonterminal Agent: $reason',
  ({ sandbox, stale, busy, reason }) => {
    const actions = agentActions(
      'ready',
      permissions,
      false,
      sandboxActionUnavailableReason(sandbox, stale, busy),
    );
    expect(actions.find((action) => action.id === 'stop_agent')).toMatchObject({
      disabled: true,
      reason: expect.stringMatching(reason),
    });
  },
);

it('does not imply Resume when runtime availability cannot be confirmed', () => {
  expect(
    agentActions('ready', permissions, false).find((action) => action.id === 'stop_agent')?.reason,
  ).toBe('Sandbox status is unavailable. Check its status.');
  const sandbox = sandboxFixture();
  expect(
    agentActions(
      'ready',
      permissions,
      sandboxChildrenAvailable(sandbox),
      sandboxActionUnavailableReason(sandbox),
    ).find((action) => action.id === 'stop_agent'),
  ).toMatchObject({ disabled: false, reason: undefined });
});
