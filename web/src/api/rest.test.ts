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
} from '../test/rest';
import { CreatedResourceReadError, createRestWebClient } from './rest';
import { profileFailure } from '../profiles/failure';

const options = { idempotencyKey: 'command-819' };

describe('public REST web binding', () => {
  it('maps public hierarchy and Agent lineage without losing nested settings', async () => {
    const test = restTransport();
    const client = createRestWebClient(test.sdk);
    test.reply({ items: [WIRE_SANDBOX_DETAIL], next: 'next' });
    expect(await client.listSandboxes({ cursor: 'first' })).toMatchObject({
      next: 'next',
      items: [{ id: 'sandbox-1', workspaceCount: 1, lastActivityAt: NOW }],
    });
    expect(test.fetch.mock.calls.at(-1)?.[0]).toBe('/api/v1/sandboxes?cursor=first');
    test.reply(WIRE_SANDBOX_DETAIL);
    expect(await client.readSandbox('sandbox-1')).toMatchObject({
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
      through_seq: 1,
      turns: [
        {
          turn_sequence: 1,
          started_at: NOW,
          owner_message: 'Review',
          agent_messages: ['Reviewed'],
          outcome: { type: 'completed', at: NOW },
        },
      ],
    });
    expect(await client.readAgentTranscript('agent-1')).toEqual({
      agentId: 'agent-1',
      throughSeq: 1,
      turns: [
        {
          turnSequence: 1,
          startedAt: NOW,
          ownerMessage: 'Review',
          agentMessages: ['Reviewed'],
          outcome: { type: 'completed', at: NOW },
        },
      ],
    });
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
