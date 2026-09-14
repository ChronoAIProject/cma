import { vi } from 'vitest';
import {
  createClient,
  type ProfileContents,
  type ProfileDocument,
  type ProfileKind,
} from '../src/index.js';

export const now = '2026-09-06T01:00:00Z';
export const reference = { id: 'profile-1', revision: 1 };
export const contents: ProfileContents = {
  agent: {
    thinking_effort: 'high',
    permission_level: 'guarded',
    skills: [],
    system_prompt: 'Review changes.',
  },
  workspace: {
    location: { type: 'directory', name: 'project' },
    name: 'project',
    setup_commands: [],
    agent_profiles: [],
  },
  sandbox: {
    cpu_millis: 1000,
    memory_mib: 1024,
    nyxid_services: [],
    agent_provider: 'codex',
    model_access: 'chronoai_managed',
    max_workspaces: 2,
    max_agents_per_workspace: 2,
    workspace_profiles: [],
    software_install_commands: [],
    environment_variables: [],
  },
};
export function profile<Kind extends ProfileKind>(
  kind: Kind,
): ProfileDocument<ProfileContents[Kind]> {
  return {
    id: `${kind}-1`,
    owner: 'owner-1',
    name: 'Project',
    description: 'A test profile',
    visibility: 'private',
    uses_count: 0,
    draft: contents[kind],
    draft_version: 1,
    created_at: now,
    updated_at: now,
  };
}
export function revision<Kind extends ProfileKind>(kind: Kind) {
  return {
    profile_id: `${kind}-1`,
    revision: 1,
    content: contents[kind],
    digest: 'digest-1',
    published_at: now,
  };
}
export function marketplaceEntry<Kind extends ProfileKind>(kind: Kind) {
  return {
    summary: {
      kind,
      id: `${kind}-1`,
      name: 'Project',
      description: 'A test profile',
      owner_display: 'Owner',
      latest_revision: 1,
      published_at: now,
      uses_count: 0,
    },
    revision: revision(kind),
    resolved_profiles: [],
    resolved_revisions: [],
    ...(kind === 'sandbox'
      ? { trust: { requires_confirmation: true, commands_digest: 'digest-1' } }
      : {}),
  };
}
export const sandbox = {
  id: 'sandbox-1',
  name: 'Project',
  status: 'active',
  profile: reference,
  limits: { max_workspaces: 2, max_agents_per_workspace: 2 },
  last_activity_at: now,
  created_at: now,
  updated_at: now,
  revision: 0,
};
export const workspace = {
  id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'Project',
  path: '/workspace/project',
  source: { type: 'empty' },
  status: 'ready',
  profile: reference,
  created_at: now,
  revision: 0,
};
export const workspaceSummary = {
  id: workspace.id,
  sandbox_id: sandbox.id,
  name: workspace.name,
  status: workspace.status,
  profile: reference,
  worktree_count: 1,
  agent_count: 1,
};
export const worktree = {
  id: 'worktree-1',
  workspace_id: workspace.id,
  sandbox_id: sandbox.id,
  name: 'Agent tree',
  path: '/workspace/trees/agent-1',
  branch: 'agent-1',
  base_commit: 'commit-1',
  status: 'ready',
  agent_id: 'agent-1',
};
export const agentProfile = {
  ...reference,
  digest: 'digest-1',
  thinking_effort: 'high',
  permission_level: 'guarded',
  skills: [],
};
export const agent = {
  id: 'agent-1',
  status: 'ready',
  profile: agentProfile,
  worktree: {
    id: worktree.id,
    name: worktree.name,
    path: worktree.path,
    branch: worktree.branch,
    base_commit: worktree.base_commit,
    status: worktree.status,
  },
  workspace: {
    id: workspace.id,
    name: workspace.name,
    path: workspace.path,
    status: workspace.status,
    profile: { ...reference, digest: 'digest-1' },
  },
  sandbox: {
    id: sandbox.id,
    name: sandbox.name,
    status: sandbox.status,
    profile: { ...reference, digest: 'digest-1' },
  },
  conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
  reach: [],
  created_at: now,
  updated_at: now,
};
export const agentSummary = {
  id: agent.id,
  sandbox_id: sandbox.id,
  workspace_id: workspace.id,
  worktree_id: worktree.id,
  status: agent.status,
  profile: agentProfile,
  created_at: now,
  updated_at: now,
};
export const resources = {
  agent_id: agent.id,
  sandbox_id: sandbox.id,
  uptime_ms: 0,
  cpu_usage_usec: null,
  cpu_millis_limit: null,
  memory_bytes_used: null,
  memory_bytes_limit: null,
  memory_bytes_peak: null,
  stall_permille: null,
  ceiling_hits: null,
  allocation_failures: null,
  kills: null,
  received_bytes: null,
  sent_bytes: null,
};
export const command = { idempotencyKey: 'command-819', requestId: 'request-819' };

export function transport() {
  const calls: { url: string; init: RequestInit }[] = [];
  let answer: unknown = {};
  let status = 200;
  const fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    return Promise.resolve(
      new Response(answer === undefined ? null : JSON.stringify(answer), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    );
  });
  const client = createClient({
    baseUrl: 'https://cma.example.test',
    fetch,
    requestIdProvider: () => 'read-819',
  });
  return {
    client,
    calls,
    fetch,
    answer: (value: unknown, code = 200) => {
      answer = value;
      status = code;
    },
  };
}
