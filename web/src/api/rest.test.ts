/** @vitest-environment node */
import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError } from '@chronoai/cma-sdk';
import {
  NOW,
  PROFILE_REF,
  restTransport,
  WIRE_AGENT,
  WIRE_SANDBOX,
  WIRE_SANDBOX_DETAIL,
  WIRE_WORKSPACE,
  WIRE_WORKSPACE_DETAIL,
  wireRevision,
} from '../test/rest';
import { CreatedResourceReadError, createRestWebClient } from './rest';
import { profileFailure } from '../profiles/failure';

const options = { idempotencyKey: 'command-819' };

describe('public REST web binding', () => {
  it('preserves exact public bootstrap lineage and unresolved worktree state without fabricating readiness', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    test.reply({
      ...WIRE_AGENT,
      status: 'starting',
      ready_at: undefined,
      worktree: { ...WIRE_AGENT.worktree, status: 'creating', base_commit: '' },
      workspace: { ...WIRE_AGENT.workspace, status: 'preparing', resolved_commit: undefined },
      sandbox: { ...WIRE_AGENT.sandbox, status: 'provisioning' },
      bootstrap: {
        operation_id: 'operation-one',
        sandbox_id: 'sandbox-1',
        workspace_id: 'workspace-1',
      },
    });
    expect(await client.readAgent('agent-1')).toMatchObject({
      id: 'agent-1',
      status: 'starting',
      readyAt: undefined,
      bootstrap: {
        operationId: 'operation-one',
        sandboxId: 'sandbox-1',
        workspaceId: 'workspace-1',
      },
      worktree: { status: 'creating', baseCommit: '' },
      workspace: { status: 'preparing', resolvedCommit: undefined },
      sandbox: { status: 'provisioning' },
    });
    test.reply(WIRE_AGENT);
    expect((await client.readAgent('agent-1')).bootstrap).toBeUndefined();
  });
  it('projects only safe access settings from the exact pinned Sandbox profile', async () => {
    const test = restTransport();
    test.reply(wireRevision('sandbox'));
    expect(
      await createRestWebClient(test.sdk).readSandboxProfileSettings({
        id: 'sandbox-1',
        revision: 1,
        digest: 'digest-1',
      }),
    ).toEqual({
      agentProvider: 'codex',
      modelAccess: 'own_credentials',
      nyxidServices: ['storage'],
    });
    test.reply({ ...wireRevision('sandbox'), revision: 2 });
    await expect(
      createRestWebClient(test.sdk).readSandboxProfileSettings({ id: 'sandbox-1', revision: 1 }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it.each(['capture', 'resume_writers'] as const)(
    'preserves public lifecycle eligibility and actual %s metadata in lists and details',
    async (phase) => {
      const test = restTransport();
      const client = createRestWebClient(test.sdk);
      const lifecycle = {
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
        revive: { allowed: false, reason: 'not_paused' },
        checkpoint: { phase, started_at: NOW, epoch: 7 },
        operation: { purpose: 'pause', phase: 'capture_and_publish_checkpoint', started_at: NOW },
      };
      const expected = {
        pause: lifecycle.pause,
        revive: lifecycle.revive,
        checkpoint: { phase, startedAt: NOW, epoch: 7 },
        operation: { purpose: 'pause', phase: 'capture_and_publish_checkpoint', startedAt: NOW },
      };
      test.reply({ items: [{ ...WIRE_SANDBOX_DETAIL, lifecycle }] });
      expect(await client.listSandboxes()).toMatchObject({ items: [{ lifecycle: expected }] });
      test.reply({ ...WIRE_SANDBOX_DETAIL, lifecycle });
      expect(await client.readSandbox('sandbox-1')).toMatchObject({ lifecycle: expected });
    },
  );

  it('keeps missing lifecycle data unknown for a mixed-version response', async () => {
    const test = restTransport();
    test.reply(WIRE_SANDBOX_DETAIL);
    expect(
      (await createRestWebClient(test.sdk).readSandbox('sandbox-1')).lifecycle,
    ).toBeUndefined();
  });

  it.each([undefined, 'digest-1'])(
    'reads configured capacity from the exact pinned revision with optional digest %s',
    async (digest) => {
      const test = restTransport();
      const client = createRestWebClient(test.sdk);
      const signal = new AbortController().signal;
      test.reply(wireRevision('sandbox'));
      expect(
        await client.readSandboxProfileCapacity(
          { id: 'sandbox-1', revision: 1, digest },
          { signal },
        ),
      ).toEqual({ cpuMillis: 2000, memoryBytes: 4096 * 1024 ** 2 });
      expect(test.fetch.mock.calls[0]?.[0]).toBe('/api/v1/sandbox-profiles/sandbox-1/revisions/1');
      expect(test.fetch.mock.calls[0]?.[1]?.signal?.aborted).toBe(false);
    },
  );

  it.each([
    { profile_id: 'other' },
    { revision: 2 },
    { digest: 'different' },
    { content: { ...wireRevision('sandbox').content, cpu_millis: 0 } },
    { content: { ...wireRevision('sandbox').content, memory_mib: Number.MAX_SAFE_INTEGER } },
  ])('refuses mismatched or invalid configured capacity: %j', async (change) => {
    const test = restTransport();
    test.reply({ ...wireRevision('sandbox'), ...change });
    await expect(
      createRestWebClient(test.sdk).readSandboxProfileCapacity({
        id: 'sandbox-1',
        revision: 1,
        digest: 'digest-1',
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it.each([403, 404])('preserves a refused pinned profile read (%s)', async (status) => {
    const test = restTransport();
    test.reply({ status, code: 'not_found', detail: 'Unavailable' }, status);
    await expect(
      createRestWebClient(test.sdk).readSandboxProfileCapacity({ id: 'sandbox-1', revision: 1 }),
    ).rejects.toBeInstanceOf(ApiError);
  });

  it('maps public hierarchy and Agent lineage without losing nested settings', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    test.reply({ items: [WIRE_SANDBOX_DETAIL], next: 'next' });
    expect(await client.listSandboxes({ cursor: 'first' })).toMatchObject({
      next: 'next',
      items: [{ id: 'sandbox-1', workspaceCount: 1, lastActivityAt: NOW }],
    });
    expect(test.fetch.mock.calls.at(-1)?.[0]).toBe('/api/v1/sandboxes?cursor=first');
    test.reply({
      ...WIRE_SANDBOX_DETAIL,
      nyxid_services: ['chrono-llm', 'ornn-api', 'api-github'],
    });
    expect(await client.readSandbox('sandbox-1')).toMatchObject({
      nyxidServices: ['chrono-llm', 'ornn-api', 'api-github'],
      limits: { maxWorkspaces: 4, maxAgentsPerWorkspace: 2 },
      runtimeVersion: 'runtime-1',
      workspaces: [{ sandboxId: 'sandbox-1', worktreeCount: 1 }],
    });
    test.reply(WIRE_WORKSPACE_DETAIL);
    expect(await client.readWorkspace('workspace-1')).toMatchObject({
      sandboxId: 'sandbox-1',
      source: WIRE_WORKSPACE.source,
      resolvedCommit: 'commit-1',
      worktrees: [{ baseCommit: 'commit-1', agentId: 'agent-1' }],
      agents: [{ workspaceId: 'workspace-1', worktreeId: 'worktree-1', title: 'Agent one' }],
    });
    test.reply(WIRE_AGENT);
    expect(await client.readAgent('agent-1')).toMatchObject({
      profile: { thinkingEffort: 'high', permissionLevel: 'guarded' },
      worktree: { workspaceId: 'workspace-1', sandboxId: 'sandbox-1', agentId: 'agent-1' },
      workspace: { resolvedCommit: 'commit-1' },
      conformsTo: WIRE_AGENT.conforms_to,
      firstMessage: { state: 'completed', queuedAt: NOW, completedAt: NOW },
      createdAt: NOW,
    });
  });

  it('retains wire event fields for the existing chronological reducer and maps snapshot fields', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    const kind = { type: 'turn_started', turn_sequence: 1 };
    test.reply({ items: [{ seq: 1, appended_at: NOW, kind }], next: 1 });
    expect(await client.readAgentEvents('agent-1', { cursor: 0 })).toEqual({
      items: [{ seq: 1, appendedAt: NOW, kind }],
      next: 1,
    });
    expect(test.fetch.mock.calls.at(-1)?.[0]).toBe('/api/v1/agents/agent-1/events?after=0');
    test.reply({
      agent_id: 'agent-1',
      through_seq: 4,
      turns: [
        {
          turn_sequence: 1,
          started_at: NOW,
          owner_message: 'Review',
          agent_messages: ['Reviewed'],
          messages: [
            { sequence: 2, role: 'user', content: 'Review', appended_at: NOW },
            { sequence: 3, role: 'assistant', content: 'Reviewed', appended_at: NOW },
            {
              sequence: 4,
              role: 'user',
              content: 'Check tests',
              appended_at: '2026-09-06T00:02:00Z',
            },
          ],
          outcome: { type: 'completed', at: NOW },
        },
      ],
    });
    expect(await client.readAgentTranscript('agent-1')).toEqual({
      agentId: 'agent-1',
      throughSeq: 4,
      turns: [
        {
          turnSequence: 1,
          startedAt: NOW,
          ownerMessage: 'Review',
          agentMessages: ['Reviewed'],
          messages: [
            { sequence: 2, role: 'user', content: 'Review', appendedAt: NOW },
            { sequence: 3, role: 'assistant', content: 'Reviewed', appendedAt: NOW },
            {
              sequence: 4,
              role: 'user',
              content: 'Check tests',
              appendedAt: '2026-09-06T00:02:00Z',
            },
          ],
          outcome: { type: 'completed', at: NOW },
        },
      ],
    });
    test.reply({ sequence: 1 });
    const history = await client.exportAgentHistory('agent-1');
    expect(await history.text()).toBe('{"sequence":1}');
    expect(test.fetch.mock.calls.at(-1)?.[0]).toBe('/api/v1/agents/agent-1/history');
    test.reply({
      agent_id: 'agent-1',
      through_seq: 1,
      input_tokens: 10,
      output_tokens: 20,
      context_used: 30,
      context_window: 100,
    });
    expect(await client.readAgentUsage('agent-1')).toEqual({
      agentId: 'agent-1',
      throughSeq: 1,
      inputTokens: 10,
      outputTokens: 20,
      contextUsed: 30,
      contextWindow: 100,
    });
    test.reply([{ sandbox_id: 'sandbox-1', seq: 2, appended_at: NOW, kind: { type: 'revived' } }]);
    expect(await client.readSandboxEvents('sandbox-1', { cursor: 1 })).toEqual([
      { sandboxId: 'sandbox-1', seq: 2, appendedAt: NOW, kind: { type: 'revived' } },
    ]);
  });

  it('creates pinned instances, then reads actual expanded details without inventing counts', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    const selection = { ...PROFILE_REF, digest: 'view-only-digest' };
    test.reply(WIRE_SANDBOX, 201);
    test.reply(WIRE_SANDBOX_DETAIL);
    expect(
      await client.createSandbox(
        { sandboxProfile: selection, trustConfirmed: true, commandsDigest: 'digest-1' },
        options,
      ),
    ).toMatchObject({ workspaceCount: 1 });
    expect(JSON.parse(String(test.fetch.mock.calls[0]?.[1]?.body))).toEqual({
      sandbox_profile: PROFILE_REF,
      trust_confirmed: true,
      commands_digest: 'digest-1',
    });
    expect(test.fetch.mock.calls[1]?.[0]).toBe('/api/v1/sandboxes/sandbox-1');
    expect(new Headers(test.fetch.mock.calls[1]?.[1]?.headers).has('idempotency-key')).toBe(false);
    test.reply(WIRE_WORKSPACE, 201);
    test.reply(WIRE_WORKSPACE_DETAIL);
    expect(
      await client.createWorkspace('sandbox-1', { workspaceProfile: selection }, options),
    ).toMatchObject({ worktreeCount: 1 });
    expect(JSON.parse(String(test.fetch.mock.calls[2]?.[1]?.body))).toEqual({
      workspace_profile: PROFILE_REF,
    });
    test.reply(WIRE_AGENT, 201);
    await client.createAgent(
      'workspace-1',
      { agentProfile: selection, firstMessage: 'Review', title: 'Research' },
      options,
    );
    expect(test.body()).toEqual({
      agent_profile: PROFILE_REF,
      first_message: 'Review',
      title: 'Research',
    });
  });

  it.each(['Sandbox', 'Workspace'] as const)(
    'preserves admitted %s identity if its detail read fails',
    async (resource) => {
      const test = restTransport();
      const client = createRestWebClient(test.sdk);
      test.reply(resource === 'Sandbox' ? WIRE_SANDBOX : WIRE_WORKSPACE, 201);
      test.reply({ status: 404, code: 'not_found', detail: 'Read projection is unavailable' }, 404);
      const result = await (
        resource === 'Sandbox'
          ? client.createSandbox({ sandboxProfile: PROFILE_REF }, options)
          : client.createWorkspace('sandbox-1', { workspaceProfile: PROFILE_REF }, options)
      ).catch((error: unknown) => error);
      expect(result).toBeInstanceOf(CreatedResourceReadError);
      expect(result).toMatchObject({
        resource,
        resourceId: `${resource.toLowerCase()}-1`,
        cause: expect.any(ApiError),
      });
      expect(profileFailure(result).uncertain).toBe(true);
      expect(test.fetch).toHaveBeenCalledTimes(2);
    },
  );

  it('binds all lifecycle commands with retained explicit keys', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    for (const method of ['pauseSandbox', 'reviveSandbox', 'deleteSandbox'] as const) {
      test.reply(WIRE_SANDBOX, 202);
      await client[method]('sandbox-1', options);
    }
    test.reply(WIRE_WORKSPACE, 202);
    await client.removeWorkspace('workspace-1', options);
    for (const method of ['interruptAgent', 'stopAgent', 'deleteAgent'] as const) {
      test.reply(undefined, 202);
      await client[method]('agent-1', options);
    }
    expect(test.fetch).toHaveBeenCalledTimes(7);
    for (const [, init] of test.fetch.mock.calls)
      expect(new Headers(init?.headers).get('idempotency-key')).toBe(options.idempotencyKey);
    expect(JSON.parse(String(test.fetch.mock.calls[5]?.[1]?.body))).toEqual({});
  });

  it('rejects malformed wire answers without using private routes', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    test.reply({ ...WIRE_AGENT, status: 'unknown' });
    await expect(client.readAgent('agent-1')).rejects.toBeInstanceOf(ProtocolError);
    expect(test.fetch).toHaveBeenCalledOnce();
  });
});
