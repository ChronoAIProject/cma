import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import {
  agentSummary,
  command,
  now,
  reference,
  sandbox,
  transport,
  workspace,
  workspaceSummary,
  worktree,
} from './resource-fixtures.js';

describe('Sandbox hierarchy resources', () => {
  it('uses the public owner lists, details and asynchronous lifecycle routes', async () => {
    const test = transport();
    const summary = { ...sandbox, workspace_count: 1, worktree_count: 1, agent_count: 1 };
    test.answer({ items: [summary], next: 'next' });
    expect(
      (await test.client.sandboxes.list({ status: 'active', cursor: 'cursor', limit: 1 })).items,
    ).toEqual([summary]);
    expect(test.calls.at(-1)?.url).toContain('/sandboxes?cursor=cursor&limit=1&status=active');
    const detail = { ...summary, workspaces: [workspaceSummary] };
    test.answer(detail);
    expect(await test.client.sandboxes.read(sandbox.id)).toEqual(detail);
    test.answer(sandbox, 201);
    const body = {
      sandbox_profile: reference,
      name: 'Project',
      trust_confirmed: true,
      commands_digest: 'digest-1',
    };
    expect(await test.client.sandboxes.create(body, command)).toEqual(sandbox);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(body);
    for (const operation of ['pause', 'revive', 'delete'] as const) {
      test.answer(sandbox, 202);
      expect(await test.client.sandboxes[operation](sandbox.id, command)).toEqual(sandbox);
      expect(test.calls.at(-1)?.init.method).toBe(operation === 'delete' ? 'DELETE' : 'POST');
      expect(new Headers(test.calls.at(-1)?.init.headers).get('idempotency-key')).toBe(
        command.idempotencyKey,
      );
    }
    const event = {
      sandbox_id: sandbox.id,
      seq: 2,
      appended_at: now,
      kind: { type: 'status_changed', from: 'paused', to: 'reviving' },
    };
    test.answer([event]);
    expect(await test.client.sandboxes.events(sandbox.id, { after: 1, limit: 5 })).toEqual([event]);
    expect(test.calls.at(-1)?.url).toContain('/events?after=1&limit=5');
    await expect(test.client.sandboxes.events('other')).rejects.toBeInstanceOf(ProtocolError);
  });

  it('creates Workspace base views and reads expanded Workspace and Worktree projections', async () => {
    const test = transport();
    test.answer({ items: [workspaceSummary] });
    expect((await test.client.workspaces.list(sandbox.id)).items).toEqual([workspaceSummary]);
    expect(test.calls.at(-1)?.url).toContain(`/sandboxes/${sandbox.id}/workspaces`);
    test.answer(workspace, 201);
    const body = {
      workspace_profile: reference,
      trust_confirmed: true,
      commands_digest: 'digest-1',
    };
    expect(await test.client.workspaces.create(sandbox.id, body, command)).toEqual(workspace);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(body);
    const hierarchyAgent = { ...agentSummary, profile: reference };
    const detail = {
      ...workspace,
      worktrees: [worktree],
      agents: [hierarchyAgent],
      worktree_count: 1,
      agent_count: 1,
    };
    test.answer(detail);
    const result = await test.client.workspaces.read(workspace.id);
    expect(result.worktrees).toEqual([worktree]);
    expect(result.agents[0]).toMatchObject({ id: 'agent-1', profile: reference });
    test.answer(workspace, 202);
    expect(await test.client.workspaces.delete(workspace.id, command)).toEqual(workspace);
    expect(test.calls.at(-1)?.init.method).toBe('DELETE');
    test.answer({ items: [worktree] });
    expect((await test.client.worktrees.list(workspace.id)).items).toEqual([worktree]);
    test.answer(worktree);
    expect(await test.client.worktrees.read(worktree.id)).toEqual(worktree);
    expect(test.calls.at(-1)?.url).toContain(`/worktrees/${worktree.id}`);
  });

  it('requires closed states, mandatory fields, safe cursors and exact statuses', async () => {
    const test = transport();
    test.answer({ ...sandbox, status: 'running' }, 201);
    await expect(
      test.client.sandboxes.create({ sandbox_profile: reference }, command),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer(workspace);
    await expect(test.client.workspaces.read(workspace.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer(sandbox);
    await expect(test.client.sandboxes.pause(sandbox.id, command)).rejects.toThrow(
      /expected HTTP 202/,
    );
    expect(() => test.client.sandboxes.list({ limit: Number.NaN })).toThrow(ValidationError);
    expect(() => test.client.sandboxes.events(sandbox.id, { after: -1 })).toThrow(ValidationError);
  });

  it('retains safe detail remediation while accepting older durable failures without it', async () => {
    const test = transport();
    const failure = {
      class: 'install_failed',
      detail: 'Software installation did not complete.',
      remediation: 'Review the install commands before creating a replacement Sandbox.',
    };
    test.answer({
      ...sandbox,
      status: 'failed',
      failure,
      workspaces: [],
      workspace_count: 0,
      worktree_count: 0,
      agent_count: 0,
    });
    expect((await test.client.sandboxes.read(sandbox.id)).failure).toEqual(failure);
    const event = {
      sandbox_id: sandbox.id,
      seq: 1,
      appended_at: now,
      kind: { type: 'failed', failure: { class: failure.class, detail: failure.detail } },
    };
    test.answer([event]);
    expect(await test.client.sandboxes.events(sandbox.id)).toEqual([event]);
    test.answer([{ ...event, kind: { ...event.kind, failure: { ...failure, remediation: 1 } } }]);
    await expect(test.client.sandboxes.events(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
  });
});
