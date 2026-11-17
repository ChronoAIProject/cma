// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../index.js';
import { now, transport } from './resource-fixtures.js';

const workspaceSummary = {
  access: 'shared',
  id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'Project',
  status: 'ready',
  worktree_count: 1,
  agent_count: 1,
};
const worktree = {
  access: 'shared',
  id: 'worktree-1',
  workspace_id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'wt-1',
  status: 'ready',
  agent_id: 'agent-1',
};
const sandbox = {
  access: 'shared',
  id: 'sandbox-1',
  name: 'Project',
  status: 'active',
  created_at: now,
  updated_at: now,
  last_activity_at: now,
  workspace_count: 1,
  worktree_count: 1,
  agent_count: 1,
  workspaces: [workspaceSummary],
};
const workspace = {
  ...workspaceSummary,
  created_at: now,
  worktrees: [worktree],
  agents: [
    {
      id: 'agent-1',
      worktree_id: worktree.id,
      workspace_id: workspaceSummary.id,
      sandbox_id: sandbox.id,
      status: 'ready',
      updated_at: now,
    },
  ],
};

describe('shared hierarchy projections', () => {
  it('returns explicit shared details and child lists while personal methods refuse the reduced shape', async () => {
    const test = transport();
    for (const [resource, body] of [
      [test.client.sandboxes, sandbox],
      [test.client.workspaces, workspace],
      [test.client.worktrees, worktree],
    ] as const) {
      test.answer(body);
      expect(await resource.readAccessible(body.id)).toEqual(body);
      test.answer(body);
      await expect(resource.read(body.id)).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer({ items: [workspaceSummary] });
    expect((await test.client.workspaces.listAccessible(sandbox.id)).items).toEqual([
      workspaceSummary,
    ]);
    expect(test.calls.at(-1)?.url).toContain('/sandboxes/sandbox-1/workspaces');
    await expect(test.client.workspaces.list(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ items: [worktree] });
    expect((await test.client.worktrees.listAccessible(workspace.id)).items).toEqual([worktree]);
    expect(test.calls.at(-1)?.url).toContain('/workspaces/workspace-1/worktrees');
    await expect(test.client.worktrees.list(workspace.id)).rejects.toBeInstanceOf(ProtocolError);
  });

  it('rejects changed identities, unknown access markers, invalid states and malformed counts', async () => {
    const test = transport();
    for (const [resource, body] of [
      [test.client.sandboxes, sandbox],
      [test.client.workspaces, workspace],
      [test.client.worktrees, worktree],
    ] as const) {
      for (const patch of [
        { id: 'another' },
        { access: 'future' },
        { access: null },
        { status: 'unknown' },
      ]) {
        test.answer({ ...body, ...patch });
        await expect(resource.readAccessible(body.id)).rejects.toBeInstanceOf(ProtocolError);
      }
    }
    for (const count of [-1, 1.5, '1']) {
      test.answer({ ...sandbox, agent_count: count });
      await expect(test.client.sandboxes.readAccessible(sandbox.id)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
  });
});

it('parses shared Sandbox inventory without accepting it as personal configuration', async () => {
  const test = transport();
  const { workspaces, ...safe } = sandbox;
  expect(workspaces).toHaveLength(1);
  test.answer({ items: [safe], next: safe.id });
  expect(
    (
      await test.client.sandboxes.listAccessible({
        organization: 'org',
        status: 'active',
        limit: 1,
      })
    ).items,
  ).toEqual([safe]);
  expect(test.calls.at(-1)?.url).toContain('/sandboxes?limit=1&status=active&organization=org');
  await expect(test.client.sandboxes.list()).rejects.toBeInstanceOf(ProtocolError);
  for (const access of ['future', null]) {
    test.answer({ items: [{ ...safe, access }] });
    await expect(
      test.client.sandboxes.listAccessible({ organization: 'org' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('accepts an optional shared detail revision while refusing invalid preconditions', async () => {
  const test = transport();
  for (const revision of [undefined, 7]) {
    const detail = { ...sandbox, ...(revision === undefined ? {} : { revision }) };
    test.answer(detail);
    expect(await test.client.sandboxes.readAccessible(sandbox.id)).toEqual(detail);
  }
  for (const revision of [0, -1, 1.5, '7']) {
    test.answer({ ...sandbox, revision });
    await expect(test.client.sandboxes.readAccessible(sandbox.id)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
});

it('pages aggregate shared Sandboxes and refuses ambiguous or untyped scope before transport', async () => {
  const test = transport();
  const { workspaces, ...summary } = sandbox;
  expect(workspaces).toHaveLength(1);
  test.answer({ items: [summary], next: summary.id });
  expect(
    await test.client.sandboxes.listAccessible({
      shared: true,
      cursor: 'sandbox-0',
      limit: 1,
      status: 'active',
    }),
  ).toEqual({ items: [summary], next: summary.id });
  expect(test.calls.at(-1)?.url).toContain(
    '/sandboxes?cursor=sandbox-0&limit=1&status=active&shared=true',
  );
  const before = test.calls.length;
  expect(() => test.client.sandboxes.listAccessible({ shared: true, organization: 'org' })).toThrow(
    ValidationError,
  );
  for (const shared of ['true', 1, null])
    expect(() => test.client.sandboxes.listAccessible({ shared: shared as never })).toThrow(
      ValidationError,
    );
  expect(test.calls).toHaveLength(before);
  test.answer({ items: [] });
  await test.client.sandboxes.listAccessible({ shared: false });
  expect(new URL(test.calls.at(-1)!.url).search).toBe('');
});
