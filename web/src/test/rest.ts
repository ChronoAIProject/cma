import {
  createClient,
  type AgentResources,
  type ProfileContents,
  type ProfileKind,
} from '@cma/client';
import { vi } from 'vitest';

export const NOW = '2026-09-06T00:00:00Z';
export const PROFILE_REF = { id: 'profile-1', revision: 1 };
export const PROFILE_CONTENTS: ProfileContents = {
  agent: {
    model: 'gpt-5',
    thinking_effort: 'high',
    permission_level: 'guarded',
    skills: [{ source: 'ornn', id: 'research', version: '1.0' }],
    system_prompt: 'Inspect the repository before making changes.',
  },
  workspace: {
    location: {
      type: 'git_repository',
      url: 'https://github.com/example/project.git',
      reference: 'main',
    },
    name: 'project',
    setup_commands: ['mkdir -p reports'],
    agent_profiles: [{ id: 'agent-1', revision: 2 }],
  },
  sandbox: {
    cpu_millis: 2000,
    memory_mib: 4096,
    nyxid_services: ['storage'],
    agent_provider: 'codex',
    model_access: 'own_credentials',
    max_workspaces: 4,
    max_agents_per_workspace: 2,
    workspace_profiles: [{ id: 'workspace-1', revision: 1 }],
    software_install_commands: ['mkdir -p /home/cma/tools'],
    environment_variables: [{ name: 'RUN_MODE', value: 'research' }],
  },
};

export function wireProfile<Kind extends ProfileKind>(kind: Kind) {
  return {
    id: `${kind}-1`,
    owner: 'owner',
    name: `Research ${kind}`,
    description: 'Repository research and development.',
    visibility: 'private',
    forked_from: PROFILE_REF,
    uses_count: 2,
    draft: PROFILE_CONTENTS[kind],
    draft_version: 3,
    created_at: NOW,
    updated_at: NOW,
    latest_revision: { revision: 1, digest: 'digest-1', published_at: NOW },
  };
}

export function wireRevision<Kind extends ProfileKind>(kind: Kind) {
  return {
    profile_id: `${kind}-1`,
    revision: 1,
    digest: 'digest-1',
    published_at: NOW,
    content: PROFILE_CONTENTS[kind],
  };
}

export function wireMarketplace<Kind extends ProfileKind>(kind: Kind) {
  return {
    summary: {
      kind,
      id: `${kind}-1`,
      name: `Research ${kind}`,
      description: 'Repository research and development.',
      owner_display: 'Research team',
      latest_revision: 1,
      published_at: NOW,
      uses_count: 2,
    },
    revision: wireRevision(kind),
    resolved_profiles: [{ kind: 'agent', id: 'agent-1', revision: 1, name: 'Research agent' }],
    resolved_revisions: (['agent', 'workspace', 'sandbox'] as const).map((kind) => ({
      kind,
      name: `Research ${kind}`,
      revision: wireRevision(kind),
    })),
    ...(kind === 'sandbox'
      ? { trust: { requires_confirmation: true, commands_digest: 'digest-1' } }
      : {}),
  };
}

export const WIRE_AGENT_RESOURCES: AgentResources = {
  agent_id: 'agt_1',
  sandbox_id: 'sbx_1',
  uptime_ms: 10000,
  cpu_usage_usec: 1000000,
  cpu_millis_limit: 4000,
  memory_bytes_used: 1024 ** 3,
  memory_bytes_limit: 8 * 1024 ** 3,
  memory_bytes_peak: null,
  stall_permille: null,
  ceiling_hits: null,
  allocation_failures: null,
  kills: null,
  received_bytes: 2048,
  sent_bytes: 1024,
};

export function wireSandboxUsage(sandboxId: string) {
  return {
    sandbox_id: sandboxId,
    owner: 'owner-1',
    name: 'Research',
    status: 'active',
    allocation: { running: true, cpu_millis: 2000, memory_mib: 4096, disk_gib: 20 },
    created_at: 1000,
    tracking_started_at: 1000,
    as_of: 1000,
    complete_lifecycle: true,
    totals: {
      uptime_seconds: 0,
      cpu_core_hours: 0,
      memory_gib_hours: 0,
      disk_gib_hours: 0,
      cpu_millis_seconds: 0,
      memory_mib_seconds: 0,
      disk_gib_seconds: 0,
    },
  };
}

export const WIRE_SANDBOX = {
  id: 'sandbox-1',
  name: 'Atlas',
  status: 'active',
  profile: PROFILE_REF,
  limits: { max_workspaces: 4, max_agents_per_workspace: 2 },
  runtime_version: 'runtime-1',
  failure: { class: 'recovered', detail: 'A previous runtime was replaced.' },
  last_activity_at: NOW,
  created_at: NOW,
  updated_at: NOW,
  revision: 1,
};
export const WIRE_WORKSPACE = {
  id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'Project',
  path: '/workspace/project',
  source: {
    type: 'git_repository',
    url: 'https://github.com/example/project.git',
    reference: 'main',
  },
  resolved_commit: 'commit-1',
  status: 'ready',
  profile: PROFILE_REF,
  created_at: NOW,
  ready_at: NOW,
  revision: 1,
};
export const WIRE_WORKSPACE_SUMMARY = {
  id: WIRE_WORKSPACE.id,
  sandbox_id: WIRE_SANDBOX.id,
  name: WIRE_WORKSPACE.name,
  status: WIRE_WORKSPACE.status,
  profile: PROFILE_REF,
  worktree_count: 1,
  agent_count: 1,
};
export const WIRE_WORKTREE = {
  id: 'worktree-1',
  workspace_id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'Agent tree',
  path: '/workspace/trees/agent-1',
  branch: 'agent-1',
  base_commit: 'commit-1',
  status: 'ready',
  agent_id: 'agent-1',
};
export const WIRE_AGENT = {
  id: 'agent-1',
  status: 'ready',
  profile: {
    ...PROFILE_REF,
    digest: 'digest-1',
    model: 'gpt-5',
    thinking_effort: 'high',
    permission_level: 'guarded',
    skills: [],
  },
  worktree: {
    id: WIRE_WORKTREE.id,
    name: WIRE_WORKTREE.name,
    path: WIRE_WORKTREE.path,
    branch: WIRE_WORKTREE.branch,
    base_commit: WIRE_WORKTREE.base_commit,
    status: WIRE_WORKTREE.status,
  },
  workspace: {
    id: WIRE_WORKSPACE.id,
    name: WIRE_WORKSPACE.name,
    path: WIRE_WORKSPACE.path,
    status: WIRE_WORKSPACE.status,
    profile: { ...PROFILE_REF, digest: 'digest-1' },
    resolved_commit: 'commit-1',
  },
  sandbox: {
    id: WIRE_SANDBOX.id,
    name: WIRE_SANDBOX.name,
    status: WIRE_SANDBOX.status,
    profile: { ...PROFILE_REF, digest: 'digest-1' },
  },
  conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
  reach: ['storage'],
  title: 'Agent one',
  first_message: { state: 'completed', queued_at: NOW, completed_at: NOW },
  created_at: NOW,
  updated_at: NOW,
  ready_at: NOW,
};
export const WIRE_SANDBOX_DETAIL = {
  ...WIRE_SANDBOX,
  workspace_count: 1,
  worktree_count: 1,
  agent_count: 1,
  workspaces: [WIRE_WORKSPACE_SUMMARY],
};
export const WIRE_WORKSPACE_DETAIL = {
  ...WIRE_WORKSPACE,
  worktree_count: 1,
  agent_count: 1,
  worktrees: [WIRE_WORKTREE],
  agents: [
    {
      id: WIRE_AGENT.id,
      worktree_id: WIRE_WORKTREE.id,
      workspace_id: WIRE_WORKSPACE.id,
      sandbox_id: WIRE_SANDBOX.id,
      status: 'ready',
      profile: PROFILE_REF,
      title: 'Agent one',
      updated_at: NOW,
    },
  ],
};

export function restTransport() {
  const replies: { body: unknown; status: number }[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>(async () => {
    const reply = replies.shift();
    if (reply === undefined) throw new Error('No test reply queued');
    return new Response(reply.body === undefined ? null : JSON.stringify(reply.body), {
      status: reply.status,
      headers: { 'content-type': 'application/json' },
    });
  });
  return {
    sdk: createClient({ fetch, requestIdProvider: () => 'request-819' }),
    fetch,
    reply: (body: unknown, status = 200) => replies.push({ body, status }),
    body: () => JSON.parse(String(fetch.mock.calls.at(-1)?.[1]?.body)) as unknown,
  };
}
