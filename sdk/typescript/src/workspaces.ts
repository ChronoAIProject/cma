import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { array, object } from './schema.js';
import { command, identity, input, read, segment, type CommandOptions } from './resources.js';
import {
  createWorkspaceRequest,
  workspace,
  workspaceDetail,
  workspaceSummary,
  worktree,
  type CreateWorkspaceRequest,
} from './instances/types.js';

/** Workspaces always belong to a Sandbox; a created Workspace is still preparing. */
export class Workspaces {
  constructor(private readonly config: ResolvedConfig) {}

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
