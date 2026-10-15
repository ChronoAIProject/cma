import type * as Wire from '@chronoai/cma-sdk';
import type {
  AgentDetail,
  AgentSummary,
  SandboxDetail,
  SandboxSummary,
  WorkspaceDetail,
  WorkspaceSummary,
  Worktree,
} from '../types';

export function sandboxSummary(value: Wire.SandboxSummary): SandboxSummary {
  return {
    id: value.id,
    owner: value.owner,
    ownerName: value.owner_name ?? undefined,
    name: value.name,
    status: value.status,
    lifecycle:
      value.lifecycle === undefined
        ? undefined
        : {
            pause: value.lifecycle.pause,
            revive: value.lifecycle.revive,
            backgroundSave:
              value.lifecycle.background_save === undefined
                ? undefined
                : {
                    phase: value.lifecycle.background_save.phase,
                    startedAt: value.lifecycle.background_save.started_at,
                    capturedAt: value.lifecycle.background_save.captured_at,
                    epoch: value.lifecycle.background_save.epoch,
                  },
            checkpoint:
              value.lifecycle.checkpoint === undefined
                ? undefined
                : {
                    phase: value.lifecycle.checkpoint.phase,
                    startedAt: value.lifecycle.checkpoint.started_at,
                    epoch: value.lifecycle.checkpoint.epoch,
                  },
            operation:
              value.lifecycle.operation === undefined
                ? undefined
                : {
                    purpose: value.lifecycle.operation.purpose,
                    phase: value.lifecycle.operation.phase,
                    startedAt: value.lifecycle.operation.started_at,
                  },
          },
    profile: value.profile,
    lastActivityAt: value.last_activity_at,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
    workspaceCount: value.workspace_count,
    worktreeCount: value.worktree_count,
    agentCount: value.agent_count,
  };
}

export function workspaceSummary(value: Wire.WorkspaceSummary): WorkspaceSummary {
  return {
    id: value.id,
    sandboxId: value.sandbox_id,
    name: value.name,
    status: value.status,
    profile: value.profile,
    worktreeCount: value.worktree_count,
    agentCount: value.agent_count,
  };
}

export function sandboxDetail(value: Wire.SandboxDetail): SandboxDetail {
  return {
    ...sandboxSummary(value),
    agentProvider: value.agent_provider,
    modelAccess: value.model_access,
    nyxidServices: value.nyxid_services,
    limits: {
      maxWorkspaces: value.limits.max_workspaces,
      maxAgentsPerWorkspace: value.limits.max_agents_per_workspace,
    },
    runtimeVersion: value.runtime_version,
    failure: value.failure,
    retiredAt: value.retired_at,
    revision: value.revision,
    workspaces: value.workspaces.map(workspaceSummary),
  };
}

export function worktree(value: Wire.Worktree): Worktree {
  return {
    id: value.id,
    workspaceId: value.workspace_id,
    sandboxId: value.sandbox_id,
    name: value.name,
    path: value.path,
    branch: value.branch,
    baseCommit: value.base_commit,
    status: value.status,
    agentId: value.agent_id,
  };
}

function agentSummary(value: Wire.HierarchyAgentSummary): AgentSummary {
  return {
    id: value.id,
    workspaceId: value.workspace_id,
    worktreeId: value.worktree_id,
    sandboxId: value.sandbox_id,
    status: value.status,
    profile: value.profile,
    title: value.title,
    runtimeMetadata:
      value.runtime_metadata === undefined
        ? undefined
        : {
            workingDirectory: value.runtime_metadata.working_directory,
            description: value.runtime_metadata.description,
          },
    updatedAt: value.updated_at,
  };
}

export function workspaceDetail(value: Wire.WorkspaceDetail): WorkspaceDetail {
  return {
    ...workspaceSummary(value),
    path: value.path,
    source: value.source,
    resolvedCommit: value.resolved_commit,
    failure: value.failure,
    createdAt: value.created_at,
    readyAt: value.ready_at,
    removedAt: value.removed_at,
    revision: value.revision,
    worktrees: value.worktrees.map(worktree),
    agents: value.agents.map(agentSummary),
  };
}

export function agentDetail(value: Wire.Agent): AgentDetail {
  return {
    id: value.id,
    status: value.status,
    bootstrap:
      value.bootstrap === undefined
        ? undefined
        : {
            operationId: value.bootstrap.operation_id,
            sandboxId: value.bootstrap.sandbox_id,
            workspaceId: value.bootstrap.workspace_id,
          },
    profile: {
      id: value.profile.id,
      revision: value.profile.revision,
      digest: value.profile.digest,
      model: value.profile.model,
      thinkingEffort: value.profile.thinking_effort,
      permissionLevel: value.profile.permission_level,
      skills: value.profile.skills,
    },
    worktree: worktree({
      ...value.worktree,
      workspace_id: value.workspace.id,
      sandbox_id: value.sandbox.id,
      agent_id: value.id,
    }),
    workspace: {
      id: value.workspace.id,
      name: value.workspace.name,
      path: value.workspace.path,
      status: value.workspace.status,
      profile: value.workspace.profile,
      resolvedCommit: value.workspace.resolved_commit,
    },
    sandbox: value.sandbox,
    conformsTo: value.conforms_to,
    reach: value.reach,
    title: value.title,
    runtimeMetadata:
      value.runtime_metadata === undefined
        ? undefined
        : {
            workingDirectory: value.runtime_metadata.working_directory,
            description: value.runtime_metadata.description,
          },
    pausedNotice: value.paused_notice,
    firstMessage:
      value.first_message === undefined
        ? undefined
        : {
            state: value.first_message.state,
            queuedAt: value.first_message.queued_at,
            completedAt: value.first_message.completed_at,
            failedAt: value.first_message.failed_at,
          },
    failure: value.failure,
    stopReason: value.stop_reason,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
    readyAt: value.ready_at,
    stoppedAt: value.stopped_at,
    deletedAt: value.deleted_at,
  };
}
