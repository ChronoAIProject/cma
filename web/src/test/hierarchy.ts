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

export function navigationFixture(
  sandboxStatus: SandboxStatus = 'active',
  agentStatus: AgentStatus = 'ready',
): readonly NavigationSandbox[] {
  return [{ sandbox: sandboxFixture(sandboxStatus), workspaces: [workspaceFixture(agentStatus)] }];
}
