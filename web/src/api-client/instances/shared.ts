import { ProtocolError } from '../errors.js';
import {
  array,
  enumeration,
  integer,
  object,
  optional,
  positiveInteger,
  record,
  text,
  type Parser,
  type Output,
} from '../schema.js';
import {
  sandboxDetail,
  sandboxSummary,
  workspaceDetail,
  workspaceSummary,
  worktree,
  sandboxStatus,
  workspaceStatus,
  worktreeStatus,
  agentStatus,
  type SandboxDetail,
  type SandboxSummary,
  type WorkspaceDetail,
  type WorkspaceSummary,
  type Worktree,
} from './types.js';

const shared = enumeration('shared');
export const sharedWorkspaceSummary = object({
  access: shared,
  id: text,
  sandbox_id: text,
  name: text,
  status: workspaceStatus,
  worktree_count: integer,
  agent_count: integer,
});
export const sharedWorktree = object({
  access: shared,
  id: text,
  workspace_id: text,
  sandbox_id: text,
  name: text,
  status: worktreeStatus,
  agent_id: optional(text),
});
export const sharedHierarchyAgent = object({
  id: text,
  worktree_id: text,
  workspace_id: text,
  sandbox_id: text,
  status: agentStatus,
  title: optional(text),
  updated_at: text,
});
export const sharedSandboxSummary = object({
  access: shared,
  id: text,
  name: text,
  status: sandboxStatus,
  created_at: text,
  updated_at: text,
  last_activity_at: text,
  retired_at: optional(text),
  workspace_count: integer,
  worktree_count: integer,
  agent_count: integer,
});
export const sharedSandboxDetail = object({
  access: shared,
  id: text,
  revision: optional(positiveInteger),
  name: text,
  status: sandboxStatus,
  created_at: text,
  updated_at: text,
  last_activity_at: text,
  retired_at: optional(text),
  workspace_count: integer,
  worktree_count: integer,
  agent_count: integer,
  workspaces: array(sharedWorkspaceSummary),
});
export const sharedWorkspaceDetail = object({
  access: shared,
  id: text,
  sandbox_id: text,
  name: text,
  status: workspaceStatus,
  created_at: text,
  ready_at: optional(text),
  removed_at: optional(text),
  worktree_count: integer,
  agent_count: integer,
  worktrees: array(sharedWorktree),
  agents: array(sharedHierarchyAgent),
});
export type SharedSandboxDetail = Output<typeof sharedSandboxDetail>;
export type SharedWorkspaceDetail = Output<typeof sharedWorkspaceDetail>;
export type SharedWorkspaceSummary = Output<typeof sharedWorkspaceSummary>;
export type SharedWorktree = Output<typeof sharedWorktree>;
export type SharedHierarchyAgent = Output<typeof sharedHierarchyAgent>;
export type AccessibleSandboxDetail =
  (SandboxDetail & { readonly access?: never }) | SharedSandboxDetail;
export type AccessibleWorkspaceDetail =
  (WorkspaceDetail & { readonly access?: never }) | SharedWorkspaceDetail;
export type AccessibleWorkspaceSummary =
  (WorkspaceSummary & { readonly access?: never }) | SharedWorkspaceSummary;
export type AccessibleWorktree = (Worktree & { readonly access?: never }) | SharedWorktree;

function projection<P, S>(value: unknown, personal: Parser<P>, shared: Parser<S>): P | S {
  const source = record(value);
  if (source.access === undefined) return personal(value);
  if (source.access === 'shared') return shared(value);
  throw new ProtocolError('unknown hierarchy access discriminator');
}
export function accessibleSandboxDetail(value: unknown): AccessibleSandboxDetail {
  return projection(value, sandboxDetail, sharedSandboxDetail);
}
export function accessibleWorkspaceDetail(value: unknown): AccessibleWorkspaceDetail {
  return projection(value, workspaceDetail, sharedWorkspaceDetail);
}
export function accessibleWorkspaceSummary(value: unknown): AccessibleWorkspaceSummary {
  return projection(value, workspaceSummary, sharedWorkspaceSummary);
}
export function accessibleWorktree(value: unknown): AccessibleWorktree {
  return projection(value, worktree, sharedWorktree);
}

export type SharedSandboxSummary = Output<typeof sharedSandboxSummary>;
export type AccessibleSandboxSummary =
  (SandboxSummary & { readonly access?: never }) | SharedSandboxSummary;
export function accessibleSandboxSummary(value: unknown): AccessibleSandboxSummary {
  return projection(value, sandboxSummary, sharedSandboxSummary);
}
