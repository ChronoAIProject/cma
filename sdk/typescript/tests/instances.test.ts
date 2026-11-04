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
  it('recovers original Workspace preparation with a keyed empty command and exact identity', async () => {
    const test = transport();
    const pending = { ...workspace, status: 'preparing' };
    for (let replay = 0; replay < 2; replay += 1) {
      test.answer(pending, 202);
      expect(await test.client.workspaces.recoverPrepare(workspace.id, command)).toEqual(pending);
      const sent = test.calls.at(-1)!;
      expect(sent.url).toContain(`/api/v1/workspaces/${workspace.id}/recover-prepare`);
      expect(sent.init.method).toBe('POST');
      expect(sent.init.body).toBeUndefined();
      expect(new Headers(sent.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
    }
    const count = test.calls.length;
    expect(() =>
      test.client.workspaces.recoverPrepare(workspace.id, { idempotencyKey: '' }),
    ).toThrow(ValidationError);
    expect(test.calls).toHaveLength(count);
    test.answer({ ...pending, id: 'another-workspace' }, 202);
    await expect(
      test.client.workspaces.recoverPrepare(workspace.id, command),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer(pending, 200);
    await expect(
      test.client.workspaces.recoverPrepare(workspace.id, command),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('accepts optional pinned providers and rejects malformed provider projections', async () => {
    const test = transport();
    const detail = {
      ...sandbox,
      workspace_count: 0,
      worktree_count: 0,
      agent_count: 0,
      workspaces: [],
    };
    for (const access of ['chronoai_managed', 'own_credentials']) {
      test.answer({ ...detail, agent_provider: 'codex', model_access: access });
      expect(await test.client.sandboxes.read(sandbox.id)).toMatchObject({
        agent_provider: 'codex',
        model_access: access,
      });
    }
    test.answer(detail);
    expect((await test.client.sandboxes.read(sandbox.id)).agent_provider).toBeUndefined();
    for (const invalid of [
      { agent_provider: 'unknown' },
      { model_access: 'unknown' },
      { model_access: null },
    ]) {
      test.answer({ ...detail, ...invalid });
      await expect(test.client.sandboxes.read(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });
  it('keeps a Sandbox colour only in its single lowercase #rrggbb spelling', async () => {
    const test = transport();
    const summary = { ...sandbox, workspace_count: 0, worktree_count: 0, agent_count: 0 };
    const detail = { ...summary, workspaces: [] };
    test.answer({ ...detail, color: '#2f96fa' });
    expect((await test.client.sandboxes.read(sandbox.id)).color).toBe('#2f96fa');
    test.answer({ items: [{ ...summary, color: '#62d079' }] });
    expect((await test.client.sandboxes.list()).items[0]).toMatchObject({ color: '#62d079' });
    test.answer(detail);
    expect((await test.client.sandboxes.read(sandbox.id)).color).toBeUndefined();
    for (const color of ['#2F96FA', '2f96fa', '#2f96f', 'blue', null, 7]) {
      test.answer({ ...detail, color });
      await expect(test.client.sandboxes.read(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });
  it('retains authoritative lifecycle eligibility and recorded phases without status inference', async () => {
    const test = transport();
    const base = {
      ...sandbox,
      workspace_count: 0,
      worktree_count: 0,
      agent_count: 0,
      workspaces: [],
    };
    for (const phase of ['capture', 'resume_writers']) {
      const lifecycle = {
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
        revive: { allowed: false, reason: 'not_paused' },
        checkpoint: { phase, started_at: now, epoch: 6 },
      };
      test.answer({ ...base, lifecycle });
      expect((await test.client.sandboxes.read(sandbox.id)).lifecycle).toEqual(lifecycle);
    }
    for (const operation of [
      { purpose: 'pause', phase: 'pause_agents', started_at: now },
      { purpose: 'revive', phase: 'conversation_reopen', started_at: now },
      { purpose: 'image_rotation', phase: 'home_restore', started_at: now },
    ]) {
      const lifecycle = {
        pause: { allowed: false, reason: 'not_active' },
        revive: { allowed: false, reason: 'not_paused' },
        operation,
      };
      test.answer({ ...base, lifecycle });
      expect((await test.client.sandboxes.read(sandbox.id)).lifecycle).toEqual(lifecycle);
    }
    test.answer(base);
    expect((await test.client.sandboxes.read(sandbox.id)).lifecycle).toBeUndefined();
    for (const malformed of [
      { pause: { allowed: true } },
      { pause: { allowed: 'yes' }, revive: { allowed: false } },
      { pause: { allowed: false, reason: 'unknown' }, revive: { allowed: false } },
      {
        pause: { allowed: false },
        revive: { allowed: false },
        checkpoint: { phase: 'unknown', started_at: now, epoch: 6 },
      },
      {
        pause: { allowed: false },
        revive: { allowed: false },
        operation: { purpose: 'pause', phase: 'home_restore', started_at: now },
      },
    ]) {
      test.answer({ ...base, lifecycle: malformed });
      await expect(test.client.sandboxes.read(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });

  it('retains explicit interruption evidence while accepting older lifecycle responses', async () => {
    const test = transport();
    const base = {
      ...sandbox,
      workspace_count: 0,
      worktree_count: 0,
      agent_count: 0,
      workspaces: [],
    };
    const lifecycle = { pause: { allowed: true }, revive: { allowed: false } };
    test.answer({ ...base, lifecycle });
    expect((await test.client.sandboxes.read(sandbox.id)).lifecycle?.interruption).toBeUndefined();
    for (const cause of ['out_of_memory', 'runtime_restarted', 'runtime_unavailable']) {
      const interruption = { cause, detected_at: now, memory_limit_mib: 8192 };
      test.answer({ ...base, lifecycle: { ...lifecycle, interruption } });
      expect((await test.client.sandboxes.read(sandbox.id)).lifecycle?.interruption).toEqual(
        interruption,
      );
    }
    for (const interruption of [
      { cause: 'exit_137', detected_at: now, memory_limit_mib: 8192 },
      { cause: 'out_of_memory', detected_at: now, memory_limit_mib: -1 },
    ]) {
      test.answer({ ...base, lifecycle: { ...lifecycle, interruption } });
      await expect(test.client.sandboxes.read(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });

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
