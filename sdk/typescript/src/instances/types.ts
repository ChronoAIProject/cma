import { modelAccess, profileReference } from '../profiles/types.js';
import { sandboxLifecycle } from './lifecycle.js';
import { sandboxRuntimeUpdate } from './runtime-update.js';
export type * from './lifecycle.js';
export type * from './runtime-update.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';

export const sandboxStatus = enumeration(
  'provisioning',
  'installing',
  'active',
  'pausing',
  'paused',
  'reviving',
  'credential_rotating',
  'rotating',
  'failed',
  'retiring',
  'retired',
);
export type SandboxStatus = Output<typeof sandboxStatus>;
export const workspaceStatus = enumeration('preparing', 'ready', 'failed', 'removing', 'removed');
export type WorkspaceStatus = Output<typeof workspaceStatus>;
export const worktreeStatus = enumeration('creating', 'ready', 'failed', 'removing', 'removed');
export type WorktreeStatus = Output<typeof worktreeStatus>;
export const agentStatus = enumeration(
  'starting',
  'ready',
  'working',
  'waiting_approval',
  'paused',
  'stopped',
  'deleted',
  'failed',
);
export type AgentStatus = Output<typeof agentStatus>;
export const failure = object({ class: text, detail: text, remediation: optional(text) });
export type Failure = Output<typeof failure>;

const sandboxFields = {
  id: text,
  owner: optional(text),
  owner_name: optional(nullable(text)),
  name: text,
  status: sandboxStatus,
  profile: profileReference,
  nyxid_services: optional(array(text)),
  limits: object({ max_workspaces: positiveInteger, max_agents_per_workspace: positiveInteger }),
  lifecycle: optional(sandboxLifecycle),
  runtime_version: optional(text),
  runtime_update: optional(sandboxRuntimeUpdate),
  failure: optional(failure),
  last_activity_at: text,
  created_at: text,
  updated_at: text,
  retired_at: optional(text),
  revision: integer,
};
const counts = { workspace_count: integer, worktree_count: integer, agent_count: integer };
export const sandbox = object(sandboxFields);
export type Sandbox = Output<typeof sandbox>;
export const sandboxSummary = object({ ...sandboxFields, ...counts });
export type SandboxSummary = Output<typeof sandboxSummary>;

export const workspaceSummary = object({
  id: text,
  sandbox_id: text,
  name: text,
  status: workspaceStatus,
  profile: profileReference,
  worktree_count: integer,
  agent_count: integer,
});
export type WorkspaceSummary = Output<typeof workspaceSummary>;
export const sandboxDetail = object({
  ...sandboxFields,
  ...counts,
  agent_provider: optional(enumeration('codex')),
  model_access: optional(modelAccess),
  workspaces: array(workspaceSummary),
});
export type SandboxDetail = Output<typeof sandboxDetail>;

export const workspaceSource = tagged({
  repository_snapshot: variant('repository_snapshot', {
    url: text,
    reference: optional(text),
    source_commit: optional(text),
  }),
  git_repository: variant('git_repository', { url: text, reference: optional(text) }),
  empty: variant('empty', {}),
});
export type WorkspaceSource = Output<typeof workspaceSource>;
const workspaceFields = {
  id: text,
  sandbox_id: text,
  name: text,
  path: text,
  source: workspaceSource,
  resolved_commit: optional(text),
  status: workspaceStatus,
  profile: profileReference,
  failure: optional(failure),
  created_at: text,
  ready_at: optional(text),
  removed_at: optional(text),
  revision: integer,
};
export const workspace = object(workspaceFields);
export type Workspace = Output<typeof workspace>;
export const worktree = object({
  id: text,
  workspace_id: text,
  sandbox_id: text,
  name: text,
  path: text,
  branch: text,
  base_commit: text,
  status: worktreeStatus,
  agent_id: optional(text),
});
export type Worktree = Output<typeof worktree>;
export const agentTaskStatus = enumeration(
  'not_started',
  'completed',
  'working',
  'waiting_approval',
  'starting',
  'paused',
  'stopped',
  'failed',
  'unknown',
);
export type AgentTaskStatus = Output<typeof agentTaskStatus>;

export const hierarchyAgentSummary = object({
  task_status: optional(agentTaskStatus),
  owner_title: optional(text),
  runtime_metadata: optional(
    object({
      working_directory: optional(text),
      description: optional(text),
      description_source: optional(enumeration('provider_title', 'prompt_preview')),
      description_is_setup_preview: optional(boolean),
    }),
  ),
  id: text,
  worktree_id: text,
  workspace_id: text,
  sandbox_id: text,
  status: agentStatus,
  profile: profileReference,
  title: optional(text),
  updated_at: text,
});
export type HierarchyAgentSummary = Output<typeof hierarchyAgentSummary>;
export const workspaceDetail = object({
  ...workspaceFields,
  worktrees: array(worktree),
  agents: array(hierarchyAgentSummary),
  worktree_count: integer,
  agent_count: integer,
});
export type WorkspaceDetail = Output<typeof workspaceDetail>;
export const createSandboxRequest = object({
  sandbox_profile: profileReference,
  name: optional(text),
  trust_confirmed: optional(boolean),
  commands_digest: optional(text),
});
export type CreateSandboxRequest = Output<typeof createSandboxRequest>;
export const grantSandboxServiceAccessRequest = object({ services: array(text) });
export type GrantSandboxServiceAccessRequest = Output<typeof grantSandboxServiceAccessRequest>;
export const createWorkspaceRequest = object({
  workspace_profile: profileReference,
  trust_confirmed: optional(boolean),
  commands_digest: optional(text),
});
export type CreateWorkspaceRequest = Output<typeof createWorkspaceRequest>;
export const createInlineWorkspaceRequest = object({
  source: tagged({
    git_repository: variant('git_repository', { url: text, reference: optional(text) }),
    empty: variant('empty', {}),
  }),
  name: optional(text),
});
export type CreateInlineWorkspaceRequest = Output<typeof createInlineWorkspaceRequest>;
