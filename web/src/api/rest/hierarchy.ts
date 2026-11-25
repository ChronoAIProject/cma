import type * as Wire from '@cma/client';
import type {
  AgentDetail,
  AgentRuntimeMetadata,
  AgentSummary,
  FirstMessageState,
  SandboxDetail,
  SandboxSummary,
  SandboxRuntimeUpdate,
  WorkspaceDetail,
  WorkspaceSummary,
  Worktree,
} from '../types';

function runtimeMetadata(
  value:
    | {
        working_directory?: string;
        description?: string;
        description_source?: AgentRuntimeMetadata['descriptionSource'];
        description_is_setup_preview?: boolean;
      }
    | undefined,
): AgentRuntimeMetadata | undefined {
  return value === undefined
    ? undefined
    : {
        workingDirectory: value.working_directory,
        description: value.description,
        descriptionSource: value.description_source,
        descriptionIsSetupPreview: value.description_is_setup_preview,
      };
}

function firstMessage(
  value:
    | (NonNullable<Wire.Agent['first_message']> & {
        purpose?: FirstMessageState['purpose'];
        turn_sequence?: number;
      })
    | undefined,
): FirstMessageState | undefined {
  return value === undefined
    ? undefined
    : {
        state: value.state,
        queuedAt: value.queued_at,
        completedAt: value.completed_at,
        failedAt: value.failed_at,
        purpose: value.purpose,
        turnSequence: value.turn_sequence,
      };
}

export function sandboxSummary(value: Wire.SandboxSummary): SandboxSummary {
  return {
    id: value.id,
    owner: value.owner,
    ownerName: value.owner_name ?? undefined,
    name: value.name,
    color: value.color,
    status: value.status,
    runtimeUpdate:
      value.runtime_update === undefined ? undefined : runtimeUpdate(value.runtime_update),
    lifecycle:
      value.lifecycle === undefined
        ? undefined
        : {
            result:
              value.lifecycle.result === undefined
                ? undefined
                : {
                    state: value.lifecycle.result.state,
                    code: value.lifecycle.result.code,
                    operationId: value.lifecycle.result.operation_id,
                    startedAt: value.lifecycle.result.started_at,
                    resultDueAt: value.lifecycle.result.result_due_at,
                    continuing: value.lifecycle.result.continuing,
                    actions: value.lifecycle.result.actions,
                    retryAfterSeconds: value.lifecycle.result.retry_after_seconds,
                  },
            pause: value.lifecycle.pause,
            revive: value.lifecycle.revive,
            interruption:
              value.lifecycle.interruption === undefined
                ? undefined
                : {
                    cause: value.lifecycle.interruption.cause,
                    detectedAt: value.lifecycle.interruption.detected_at,
                    memoryLimitMib: value.lifecycle.interruption.memory_limit_mib,
                  },
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
    alwaysOn: value.always_on,
    lastActivityAt: value.last_activity_at,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
    workspaceCount: value.workspace_count,
    worktreeCount: value.worktree_count,
    agentCount: value.agent_count,
  };
}

export function runtimeUpdate(value: Wire.SandboxRuntimeUpdate): SandboxRuntimeUpdate {
  return {
    currentVersion: value.current_version,
    currentVersionConfirmed: value.current_version_confirmed,
    imageOrigin: value.image_origin,
    latestVersion: value.latest_version,
    updateAvailable: value.update_available,
    state: value.state,
    canUpdate: value.can_update,
    blockedReason: value.blocked_reason,
    targetVersion: value.target_version,
    requestedAt: value.requested_at,
    startedAt: value.started_at,
    completedAt: value.completed_at,
    failure: value.failure,
    schedule: { timezone: value.schedule.timezone, localTime: value.schedule.local_time },
    preservesHome: value.preserves_home,
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
    taskStatus: value.task_status,
    taskAcceptance: value.task_acceptance,
    taskStartedAt: value.task_started_at,
    taskActivityAt: value.task_activity_at,
    id: value.id,
    workspaceId: value.workspace_id,
    worktreeId: value.worktree_id,
    sandboxId: value.sandbox_id,
    status: value.status,
    profile: value.profile,
    title: value.title,
    runtimeMetadata: runtimeMetadata(value.runtime_metadata),
    ownerTitle: value.owner_title,
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
    taskAcceptance: value.task_acceptance,
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
      source: value.workspace.source,
    },
    sandbox: value.sandbox,
    desktop: value.desktop,
    conformsTo: value.conforms_to,
    reach: value.reach,
    title: value.title,
    runtimeMetadata: runtimeMetadata(value.runtime_metadata),
    ownerTitle: value.owner_title,
    pausedNotice: value.paused_notice,
    firstMessage: firstMessage(value.first_message),
    failure: value.failure,
    stopReason: value.stop_reason,
    restartAvailable: value.restart_available,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
    readyAt: value.ready_at,
    stoppedAt: value.stopped_at,
    deletedAt: value.deleted_at,
  };
}
