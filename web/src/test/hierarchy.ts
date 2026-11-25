import type { AgentStatus, SandboxDetail, SandboxStatus, WorkspaceDetail } from '../api/types';
import type { NavigationSandbox } from '../shell/navigation';

export function workspaceFixture(agentStatus: AgentStatus = 'ready'): WorkspaceDetail {
  return {
    id: 'wsp_1',
    sandboxId: 'sbx_1',
    name: 'chrono-bot',
    status: 'ready',
    profile: { id: 'wkp_default', revision: 2 },
    worktreeCount: 1,
    agentCount: 1,
    path: '/workspaces/chrono-bot',
    source: { type: 'git_repository', url: 'https://example.test/chrono-bot.git' },
    resolvedCommit: 'abc123',
    createdAt: '2026-09-06T00:00:00Z',
    readyAt: '2026-09-06T00:01:00Z',
    revision: 3,
    worktrees: [
      {
        id: 'wkt_1',
        workspaceId: 'wsp_1',
        sandboxId: 'sbx_1',
        name: 'agent-one',
        path: '/worktrees/agent-one',
        branch: 'agent/one',
        baseCommit: 'abc123',
        status: 'ready',
        agentId: 'agt_1',
      },
    ],
    agents: [
      {
        id: 'agt_1',
        worktreeId: 'wkt_1',
        workspaceId: 'wsp_1',
        sandboxId: 'sbx_1',
        status: agentStatus,
        profile: { id: 'agp_default', revision: 4 },
        title: 'Agent one',
        updatedAt: '2026-09-06T00:05:00Z',
        taskActivityAt: '2026-09-06T00:05:00Z',
      },
    ],
  };
}

export function sandboxFixture(status: SandboxStatus = 'active'): SandboxDetail {
  const workspace = workspaceFixture();
  return {
    id: 'sbx_1',
    name: 'Atlas',
    status,
    lifecycle: {
      pause: status === 'active' ? { allowed: true } : { allowed: false, reason: 'not_active' },
      revive: status === 'paused' ? { allowed: true } : { allowed: false, reason: 'not_paused' },
    },
    profile: { id: 'sbp_default', revision: 1 },
    lastActivityAt: '2026-09-06T00:05:00Z',
    createdAt: '2026-09-06T00:00:00Z',
    updatedAt: '2026-09-06T00:05:00Z',
    workspaceCount: 1,
    worktreeCount: 1,
    agentCount: 1,
    limits: { maxWorkspaces: 4, maxAgentsPerWorkspace: 4 },
    runtimeVersion: '1.2.3',
    revision: 5,
    workspaces: [workspace],
  };
}

export function delayedRetirementFixture(): SandboxDetail {
  const sandbox = sandboxFixture('retiring');
  return {
    ...sandbox,
    lifecycle: {
      ...sandbox.lifecycle!,
      result: {
        state: 'recoverable_pending',
        code: 'lifecycle_wait_exceeded',
        operationId: 'retirement-fixture',
        startedAt: '2026-09-06T00:00:00Z',
        resultDueAt: '2026-09-06T00:01:30Z',
        continuing: true,
        actions: ['observe'],
        retryAfterSeconds: 5,
      },
    },
  };
}

export function navigationFixture(
  sandboxStatus: SandboxStatus = 'active',
  agentStatus: AgentStatus = 'ready',
): readonly NavigationSandbox[] {
  return [{ sandbox: sandboxFixture(sandboxStatus), workspaces: [workspaceFixture(agentStatus)] }];
}

export function parkedRuntimeUpdateFixture(): SandboxDetail {
  const sandbox = sandboxFixture('rotating');
  return {
    ...sandbox,
    lifecycle: {
      ...sandbox.lifecycle!,
      operation: {
        purpose: 'image_rotation',
        phase: 'conversation_reopen',
        startedAt: '2026-11-19T00:00:00Z',
      },
    },
    runtimeUpdate: {
      currentVersion: 'image-before',
      latestVersion: 'image-after',
      targetVersion: 'image-after',
      updateAvailable: true,
      state: 'failed',
      canUpdate: true,
      failure: { code: 'runtime_unavailable', message: 'The runtime could not be reopened.' },
      schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
      preservesHome: true,
    },
  };
}

export function parkedRuntimeResultFixture(
  purpose: 'image_rotation' | 'revive' = 'image_rotation',
): SandboxDetail {
  const base = parkedRuntimeUpdateFixture();
  return {
    ...base,
    status: purpose === 'revive' ? 'reviving' : 'rotating',
    runtimeUpdate: undefined,
    lifecycle: {
      ...base.lifecycle!,
      operation: { ...base.lifecycle!.operation!, purpose },
      result: {
        state: 'failed',
        code: 'runtime_update_parked',
        operationId: 'update-operation',
        startedAt: base.lifecycle!.operation!.startedAt,
        continuing: false,
        actions: ['observe'],
      },
    },
  };
}
