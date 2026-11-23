import { expect, it } from 'vitest';
import type { SandboxSummary } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import type { MenuEntry, MenuItem } from '../ui/Menu';
import { parkedRuntimeUpdateFixture, sandboxFixture } from '../test/hierarchy';
import { sandboxCreateReason, sandboxHeaderMenu } from './sandboxHeaderActions';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};
const settled = { busy: false, stale: false, needsConnections: false };

it('allows deleting a parked runtime update despite retained lifecycle work', () => {
  const menu = sandboxHeaderMenu(
    parkedRuntimeUpdateFixture(),
    permissions,
    { ...settled, lifecycleBusy: true },
    false,
  );
  expect(row(menu, 'delete_sandbox')).toMatchObject({ disabled: false });
  expect(row(menu, 'new_agent')).toMatchObject({ disabled: true });
  expect(row(menu, 'new_workspace')).toMatchObject({ disabled: true });
});

it.each([
  ['stale status', { ...settled, stale: true }],
  ['a local submission', { ...settled, busy: true }],
] as const)('keeps parked-update Delete disabled for %s', (_label, context) => {
  expect(
    row(
      sandboxHeaderMenu(parkedRuntimeUpdateFixture(), permissions, context, false),
      'delete_sandbox',
    ),
  ).toMatchObject({ disabled: true });
});

it.each(['checkpoint', 'updating', 'queued', 'other-operation', 'missing-update'] as const)(
  'does not treat %s as a deletable parked update',
  (condition) => {
    const parked = parkedRuntimeUpdateFixture();
    const sandbox: SandboxSummary = {
      ...parked,
      lifecycle: {
        ...parked.lifecycle!,
        checkpoint:
          condition === 'checkpoint'
            ? { phase: 'capture', epoch: 1, startedAt: '2026-11-19T00:00:00Z' }
            : undefined,
        operation: {
          ...parked.lifecycle!.operation!,
          purpose: condition === 'other-operation' ? 'pause' : 'image_rotation',
        },
      },
      runtimeUpdate:
        condition === 'missing-update'
          ? undefined
          : {
              ...parked.runtimeUpdate!,
              state: condition === 'updating' || condition === 'queued' ? condition : 'failed',
            },
    };
    expect(
      row(sandboxHeaderMenu(sandbox, permissions, settled, false), 'delete_sandbox'),
    ).toMatchObject({ disabled: true });
  },
);

it('requires deletion permission even for a parked update', () => {
  expect(
    row(
      sandboxHeaderMenu(
        parkedRuntimeUpdateFixture(),
        { ...permissions, deleteSandbox: false },
        settled,
        false,
      ),
      'delete_sandbox',
    ),
  ).toBeUndefined();
});
const pausing: SandboxSummary = {
  ...sandboxFixture(),
  lifecycle: {
    pause: { allowed: false, reason: 'work_in_progress' },
    revive: { allowed: false, reason: 'not_paused' },
    operation: { purpose: 'pause', phase: 'capture', startedAt: '2026-10-21T00:00:00Z' },
  },
};

function rows(entries: readonly MenuEntry[]) {
  return entries.map((entry) => ('type' in entry ? entry.type : entry.id));
}
function row(entries: readonly MenuEntry[], id: string) {
  return entries.find((entry): entry is MenuItem => !('type' in entry) && entry.id === id);
}

it('orders routine rows first and Delete Sandbox last behind a separator', () => {
  expect(rows(sandboxHeaderMenu(sandboxFixture(), permissions, settled, true))).toEqual([
    'new_agent',
    'new_workspace',
    'sharing',
    'technical_details',
    'separator',
    'delete_sandbox',
  ]);
  expect(
    row(sandboxHeaderMenu(sandboxFixture(), permissions, settled, true), 'delete_sandbox'),
  ).toMatchObject({ label: 'Delete Sandbox', danger: true, disabled: false });
});

it('lists Sharing only with current sharing authority and omits rows without permission', () => {
  expect(
    rows(
      sandboxHeaderMenu(
        sandboxFixture(),
        { ...permissions, createAgent: false, createWorkspace: false, deleteSandbox: false },
        settled,
        false,
      ),
    ),
  ).toEqual(['technical_details']);
});

it('keeps unavailable rows in place with the reason they are unavailable', () => {
  const menu = sandboxHeaderMenu(pausing, permissions, settled, false);
  for (const id of ['new_agent', 'new_workspace', 'delete_sandbox'])
    expect(row(menu, id)).toMatchObject({
      disabled: true,
      reason: 'Wait for the current Sandbox operation to finish.',
    });
  expect(row(menu, 'technical_details')?.disabled).toBeUndefined();
  expect(
    row(
      sandboxHeaderMenu(sandboxFixture('retiring'), permissions, settled, false),
      'delete_sandbox',
    )?.reason,
  ).toBe('Deletion is already in progress.');
  expect(
    row(
      sandboxHeaderMenu(sandboxFixture(), permissions, { ...settled, busy: true }, false),
      'delete_sandbox',
    )?.reason,
  ).toBe('Wait for the current Sandbox operation to finish.');
});

it.each([
  ['an available Sandbox', sandboxFixture(), settled, undefined],
  [
    'a stale observation',
    sandboxFixture(),
    { ...settled, stale: true },
    'Checking the current Sandbox status.',
  ],
  ['a recorded operation', pausing, settled, 'Wait for the current Sandbox operation to finish.'],
  [
    'a paused Sandbox',
    sandboxFixture('paused'),
    settled,
    'Agents and Workspaces can start when the Sandbox is active.',
  ],
  [
    'unreported eligibility',
    { ...sandboxFixture(), lifecycle: undefined },
    settled,
    'Agents and Workspaces can start when the Sandbox is active.',
  ],
  [
    'another pending action',
    sandboxFixture(),
    { ...settled, busy: true },
    'Wait for the current Sandbox operation to finish.',
  ],
  [
    'an accepted lifecycle request awaiting a fresh observation',
    sandboxFixture(),
    { ...settled, lifecycleBusy: true },
    'Wait for the current Sandbox operation to finish.',
  ],
  [
    'unconfirmed connections',
    sandboxFixture(),
    { ...settled, needsConnections: true },
    'Checking your service connections.',
  ],
] as const)('explains creation availability for %s', (_case, sandbox, context, reason) => {
  expect(sandboxCreateReason(sandbox, context)).toBe(reason);
});
