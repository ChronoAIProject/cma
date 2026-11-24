import { describe, expect, it } from 'vitest';
import type { AgentStatus, SandboxDetail } from '../api/types';
import {
  parkedRuntimeResultFixture,
  parkedRuntimeUpdateFixture,
  sandboxFixture,
} from '../test/hierarchy';
import type { TreePermissions } from './Tree';
import type { TreeRowAction } from './TreeRow';
import { agentActions, type AgentActionContext } from './treeActions';

const owner = 'sandbox-owner';
const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: true,
  removeWorkspace: false,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: false,
};

function parked(purpose: 'image_rotation' | 'revive' = 'image_rotation'): SandboxDetail {
  return { ...parkedRuntimeResultFixture(purpose), owner };
}

function retireableRevive(): SandboxDetail {
  const sandbox = parked('revive');
  return {
    ...sandbox,
    lifecycle: {
      ...sandbox.lifecycle!,
      result: { ...sandbox.lifecycle!.result!, actions: ['observe', 'retire'] },
    },
  };
}

function expectDeletionBlocked(actions: TreeRowAction[], reason?: RegExp) {
  const deletion = actions.find((action) => action.id === 'delete_sandbox');
  expect(actions.some((action) => action.id === 'delete_sandbox' && !action.disabled)).toBe(false);
  if (deletion !== undefined) {
    expect(deletion.disabled).toBe(true);
    expect(deletion.reason).toEqual(expect.stringMatching(reason ?? /\S/));
  }
}

describe('parked task environment deletion eligibility', () => {
  const eligible = [
    { name: 'list-only ImageRotation', sandbox: parked() },
    { name: 'legacy detail ImageRotation', sandbox: { ...parkedRuntimeUpdateFixture(), owner } },
    { name: 'managed Revive with retire action', sandbox: retireableRevive() },
  ];

  it.each(eligible)('offers a distinct environment action for $name', ({ sandbox }) => {
    for (const status of ['ready', 'working', 'paused', 'stopped', 'failed'] as const) {
      const actions = agentActions(status, permissions, { sandbox, viewerId: owner });
      const deletion = actions.find((action) => action.id === 'delete_sandbox');
      expect(deletion).toMatchObject({ label: 'Delete environment…', danger: true });
      expect(deletion?.disabled).not.toBe(true);
      expect(deletion?.reason).toBeUndefined();
      expect(actions.filter((action) => action.id === 'delete_sandbox')).toHaveLength(1);
      expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
        label: 'Delete',
        disabled: true,
      });
      expect(actions.find((action) => action.id === 'stop_agent')).toMatchObject({
        disabled: true,
      });
    }
  });

  it.each([
    { deleteSandbox: true, deleteAgent: false },
    { deleteSandbox: true, deleteAgent: true },
    { deleteSandbox: false, deleteAgent: true },
    { deleteSandbox: false, deleteAgent: false },
  ])('keeps Sandbox and Agent permissions independent: %j', (grants) => {
    const actions = agentActions(
      'working',
      { ...permissions, ...grants, stopAgent: false },
      {
        sandbox: parked(),
        viewerId: owner,
      },
    );
    const deletion = actions.find((action) => action.id === 'delete_sandbox');
    if (grants.deleteSandbox) {
      expect(deletion).toMatchObject({ label: 'Delete environment…', danger: true });
      expect(deletion?.disabled).not.toBe(true);
    } else {
      expect(deletion).toBeUndefined();
    }
    if (grants.deleteAgent) {
      expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
        disabled: true,
      });
    } else {
      expect(actions.find((action) => action.id === 'delete_agent')).toBeUndefined();
    }
    expect(actions.find((action) => action.id === 'stop_agent')).toBeUndefined();
  });

  it.each([
    { name: 'foreign viewer', context: { sandbox: parked(), viewerId: 'another-owner' } },
    { name: 'unknown viewer', context: { sandbox: parked() } },
    {
      name: 'unknown owner',
      context: { sandbox: { ...parked(), owner: undefined }, viewerId: owner },
    },
    { name: 'unknown owner and viewer', context: { sandbox: { ...parked(), owner: undefined } } },
    { name: 'missing parent', context: { viewerId: owner } },
  ] satisfies { name: string; context: AgentActionContext }[])(
    'does not expose environment deletion to $name',
    ({ context }) => {
      const actions = agentActions('stopped', permissions, context);
      expect(actions.find((action) => action.id === 'delete_sandbox')).toBeUndefined();
      expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
        disabled: true,
      });
    },
  );

  it.each(['image_rotation', 'revive'] as const)(
    'does not let parked %s bypass stale, submission or checkpoint guards',
    (purpose) => {
      const sandbox = purpose === 'revive' ? retireableRevive() : parked();
      const checkpoint: SandboxDetail = {
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          checkpoint: { phase: 'capture', epoch: 2, startedAt: '2026-11-19T00:01:00Z' },
        },
      };
      for (const context of [
        { sandbox, viewerId: owner, stale: true },
        { sandbox, viewerId: owner, busy: true },
        { sandbox: checkpoint, viewerId: owner },
      ]) {
        const actions = agentActions('stopped', permissions, context);
        expectDeletionBlocked(actions);
        expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
          disabled: true,
        });
      }
    },
  );

  it('does not treat a parked Revive observation as permission to retire', () => {
    const sandbox = parked('revive');
    const actions = agentActions('failed', permissions, { sandbox, viewerId: owner });
    expectDeletionBlocked(actions, /operation|resum/i);
    expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
      disabled: true,
    });
    const retry = actions.find((action) => action.id === 'open_runtime_update');
    expect(retry).toMatchObject({ route: { kind: 'sandbox', sandboxId: sandbox.id } });
    expect(retry?.disabled).not.toBe(true);
  });

  const legacy = { ...parkedRuntimeUpdateFixture(), owner };
  const normalRotation: SandboxDetail = {
    ...parked(),
    lifecycle: { ...parked().lifecycle!, result: undefined },
  };
  it.each([
    { name: 'normal rotation', sandbox: normalRotation },
    {
      name: 'ordinary Revive',
      sandbox: {
        ...parked('revive'),
        lifecycle: { ...parked('revive').lifecycle!, result: undefined },
      },
    },
    {
      name: 'historical failure after Active',
      sandbox: { ...legacy, status: 'active' as const, lifecycle: sandboxFixture().lifecycle },
    },
    {
      name: 'completed old-server request',
      sandbox: {
        ...legacy,
        runtimeUpdate: { ...legacy.runtimeUpdate!, completedAt: '2026-11-19T00:02:00Z' },
      },
    },
    {
      name: 'completed request with parked result',
      sandbox: {
        ...parked(),
        runtimeUpdate: { ...legacy.runtimeUpdate!, completedAt: '2026-11-19T00:02:00Z' },
      },
    },
    {
      name: 'old-server failure from another admission',
      sandbox: {
        ...legacy,
        runtimeUpdate: { ...legacy.runtimeUpdate!, startedAt: '2026-11-18T00:00:00Z' },
      },
    },
    { name: 'unrelated failed Sandbox', sandbox: { ...sandboxFixture('failed'), owner } },
    { name: 'healthy Active Sandbox', sandbox: { ...sandboxFixture(), owner } },
  ])('does not invent parked environment deletion for $name', ({ sandbox }) => {
    const actions = agentActions('stopped', permissions, { sandbox, viewerId: owner });
    expect(actions.find((action) => action.id === 'delete_sandbox')).toBeUndefined();
    expect(actions.find((action) => action.id === 'open_runtime_update')).toBeUndefined();
  });

  it.each(['retiring', 'retired'] as const)('keeps %s ahead of stale parked fields', (status) => {
    const actions = agentActions('failed', permissions, {
      sandbox: { ...parked(), status },
      viewerId: owner,
    });
    expectDeletionBlocked(actions);
    expect(actions.find((action) => action.id === 'open_runtime_update')).toBeUndefined();
    expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
      disabled: true,
      reason: expect.stringMatching(/delet/i),
    });
  });

  it.each(['image_rotation', 'revive'] as const)(
    'gives parked %s Stop and Agent Delete a reachable runtime recovery hint',
    (purpose) => {
      const sandbox = parked(purpose);
      const actions = agentActions(
        'working',
        { ...permissions, deleteSandbox: false },
        { sandbox, viewerId: owner },
      );
      const retry = actions.find((action) => action.id === 'open_runtime_update');
      expect(retry).toMatchObject({
        label: 'Retry runtime update',
        route: { kind: 'sandbox', sandboxId: sandbox.id },
      });
      expect(retry?.disabled).not.toBe(true);
      for (const id of ['stop_agent', 'delete_agent']) {
        const action = actions.find((item) => item.id === id);
        expect(action).toMatchObject({
          disabled: true,
          reason: expect.stringContaining('Retry runtime update'),
        });
        expect(action?.reason).not.toMatch(
          /available when.*active|finish resuming|delete environment/i,
        );
      }
    },
  );

  it.each([
    {
      purpose: 'image_rotation',
      stale: true,
      busy: false,
      reason: /status|refresh|checking|unavailable|reconnect/i,
    },
    {
      purpose: 'revive',
      stale: true,
      busy: false,
      reason: /status|refresh|checking|unavailable|reconnect/i,
    },
    { purpose: 'image_rotation', stale: false, busy: true, reason: /action|confirm/i },
    { purpose: 'revive', stale: false, busy: true, reason: /action|confirm/i },
  ] as const)(
    'prioritizes stale=$stale and busy=$busy over parked $purpose recovery hints',
    ({ purpose, stale, busy, reason }) => {
      const actions = agentActions('working', permissions, {
        sandbox: parked(purpose),
        viewerId: owner,
        stale,
        busy,
      });
      expectDeletionBlocked(actions);
      for (const id of ['stop_agent', 'delete_agent']) {
        const action = actions.find((item) => item.id === id);
        expect(action).toMatchObject({ disabled: true, reason: expect.stringMatching(reason) });
        expect(action?.reason).not.toMatch(/Retry runtime update|finish resuming/i);
      }
    },
  );

  it.each<AgentStatus>(['ready', 'working', 'stopped', 'failed'])(
    'preserves ordinary Agent actions for %s in an Active Sandbox',
    (status) => {
      const actions = agentActions(status, permissions, {
        sandbox: { ...sandboxFixture(), owner },
        viewerId: owner,
      });
      expect(actions.find((action) => action.id === 'delete_sandbox')).toBeUndefined();
      expect(actions.find((action) => action.id === 'open_runtime_update')).toBeUndefined();
      const terminal = status === 'stopped' || status === 'failed';
      expect(actions.find((action) => action.id === 'delete_agent')).toMatchObject({
        label: 'Delete',
        disabled: false,
      });
      expect(actions.find((action) => action.id === 'stop_agent')).toMatchObject({
        disabled: terminal,
      });
    },
  );
});
