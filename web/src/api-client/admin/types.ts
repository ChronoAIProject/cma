import { agentStatus, failure, sandboxStatus, workspaceStatus } from '../instances/types.js';
import { sandboxLifecycle } from '../instances/lifecycle.js';
import { sandboxRuntimeUpdate } from '../instances/runtime-update.js';
import { profileKind, profileVisibility } from '../profiles/types.js';
import {
  array,
  boolean,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
  type Parser,
} from '../schema.js';

export function adminPage<T>(parse: Parser<T>) {
  return object({ items: array(parse), next: nullable(text) });
}

export const adminSandboxSummary = object({
  id: text,
  owner: text,
  owner_name: optional(nullable(text)),
  name: text,
  status: sandboxStatus,
  runtime_version: nullable(text),
  agent_count: integer,
  last_activity_at: text,
  retired_at: nullable(text),
});
export type AdminSandboxSummary = Output<typeof adminSandboxSummary>;

// The admin wire carries a core record; deliberately project no credential or checkpoint state.
export const adminSandbox = object({
  id: text,
  owner: text,
  name: text,
  status: sandboxStatus,
  profile: object({
    id: text,
    revision: positiveInteger,
    digest: text,
    always_on: optional(boolean),
  }),
  runtime: optional(object({ runtime_version: text })),
  limits: object({ max_workspaces: positiveInteger, max_agents_per_workspace: positiveInteger }),
  failure: optional(failure),
  last_activity_at: text,
  created_at: text,
  updated_at: text,
  retired_at: optional(text),
  revision: positiveInteger,
});
export type AdminSandbox = Output<typeof adminSandbox>;
const adminWorkspace = object({
  id: text,
  name: text,
  status: workspaceStatus,
  profile_id: text,
  profile_revision: positiveInteger,
});
const adminAgent = object({
  id: text,
  workspace_id: text,
  worktree_id: text,
  status: agentStatus,
  profile_id: text,
  profile_revision: positiveInteger,
  title: nullable(text),
  updated_at: text,
});
export const adminSandboxDetail = object({
  owner_name: optional(nullable(text)),
  lifecycle: optional(sandboxLifecycle),
  runtime_update: optional(nullable(sandboxRuntimeUpdate)),
  sandbox: adminSandbox,
  workspaces: array(adminWorkspace),
  agents: array(adminAgent),
});
export type AdminSandboxDetail = Output<typeof adminSandboxDetail>;

export const adminProfileSummary = object({
  kind: profileKind,
  id: text,
  owner: text,
  owner_name: optional(nullable(text)),
  name: text,
  visibility: profileVisibility,
  updated_at: text,
  retired_at: nullable(text),
});
export type AdminProfileSummary = Output<typeof adminProfileSummary>;

export const activityAction = enumeration(
  'authenticate',
  'create',
  'read',
  'list',
  'update',
  'delete',
  'configure',
  'gateway_record_write',
  'gateway_record_read',
  'gateway_record_delete',
);
export type ActivityAction = Output<typeof activityAction>;
const activityTarget = object({
  kind: enumeration('subject', 'service', 'skill', 'resource'),
  id: text,
});
const activityOutcome = tagged(
  {
    succeeded: object({ status: enumeration('succeeded') }),
    refused: object({ status: enumeration('refused'), code: text }),
    failed: object({ status: enumeration('failed'), code: text }),
  },
  'status',
);
const activityFields = {
  id: text,
  at: text,
  actor: nullable(text),
  actor_name: optional(nullable(text)),
  actor_reference: optional(text),
  target_name: optional(nullable(text)),
  action: activityAction,
  target: nullable(activityTarget),
  outcome: activityOutcome,
};
export const activitySummary = object(activityFields);
export type ActivitySummary = Output<typeof activitySummary>;
export const activityRecord = object({ ...activityFields, detail: dictionary(text) });
export type ActivityRecord = Output<typeof activityRecord>;
