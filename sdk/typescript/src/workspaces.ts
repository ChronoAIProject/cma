import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import {
  accessibleWorkspaceDetail,
  accessibleWorkspaceSummary,
  accessibleWorktree,
} from './instances/shared.js';
import { array, object } from './schema.js';
import { command, identity, input, read, segment, type CommandOptions } from './resources.js';
import {
  createWorkspaceRequest,
  workspace,
  workspaceDetail,
  workspaceSummary,
  worktree,
  createInlineWorkspaceRequest,
  type CreateInlineWorkspaceRequest,
  type CreateWorkspaceRequest,
} from './instances/types.js';

/** Workspaces always belong to a Sandbox; a created Workspace is still preparing. */
export class Workspaces {
  constructor(private readonly config: ResolvedConfig) {}

  /** Requires visibility of the parent Sandbox under current policy. */
  listAccessible(sandboxId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(sandboxId)}/workspaces`,
      object({ items: array(accessibleWorkspaceSummary) }),
      options,
    );
  }
  readAccessible(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/workspaces/${segment(id)}`,
      identity(accessibleWorkspaceDetail, id),
      options,
    );
  }

  list(sandboxId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(sandboxId)}/workspaces`,
      object({ items: array(workspaceSummary) }),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/workspaces/${segment(id)}`,
      identity(workspaceDetail, id),
      options,
    );
  }

  create(sandboxId: string, body: CreateWorkspaceRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/sandboxes/${segment(sandboxId)}/workspaces`, workspace, {
      ...command(options),
      method: 'POST',
      body: input(createWorkspaceRequest, body),
      expectedStatus: 201,
    });
  }

  /** Admit a workspace from an inline source selected in Agent Manager. */
  createInline(sandboxId: string, body: CreateInlineWorkspaceRequest, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(sandboxId)}/workspaces/inline`,
      workspace,
      {
        ...command(options),
        method: 'POST',
        body: input(createInlineWorkspaceRequest, body),
        expectedStatus: 201,
      },
    );
  }

  /** Preserve original identity/data and retry preparation of a failed, never-ready workspace. */
  recoverPrepare(id: string, options: CommandOptions) {
    return read(this.config, `/api/v1/workspaces/${segment(id)}/recover-prepare`, identity(workspace, id), {
      ...command(options),
      method: 'POST',
      expectedStatus: 202,
    });
  }

  delete(id: string, options: CommandOptions) {
    return read(this.config, `/api/v1/workspaces/${segment(id)}`, identity(workspace, id), {
      ...command(options),
      method: 'DELETE',
      expectedStatus: 202,
    });
  }
}

/** Read-only owner projection of the Worktrees allocated for Agents. */
export class Worktrees {
  constructor(private readonly config: ResolvedConfig) {}

  /** Requires visibility of the parent Workspace under current policy. */
  listAccessible(workspaceId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/workspaces/${segment(workspaceId)}/worktrees`,
      object({ items: array(accessibleWorktree) }),
      options,
    );
  }
  readAccessible(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/worktrees/${segment(id)}`,
      identity(accessibleWorktree, id),
      options,
    );
  }

  list(workspaceId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/workspaces/${segment(workspaceId)}/worktrees`,
      object({ items: array(worktree) }),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}) {
    return read(this.config, `/api/v1/worktrees/${segment(id)}`, identity(worktree, id), options);
  }
}
