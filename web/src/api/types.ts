export type SandboxStatus =
  | 'provisioning'
  | 'installing'
  | 'active'
  | 'pausing'
  | 'paused'
  | 'reviving'
  | 'credential_rotating'
  | 'rotating'
  | 'failed'
  | 'retiring'
  | 'retired';

export type WorkspaceStatus = 'preparing' | 'ready' | 'failed' | 'removing' | 'removed';
export type WorktreeStatus = 'creating' | 'ready' | 'failed' | 'removing' | 'removed';
export type AgentStatus =
  | 'starting'
  | 'ready'
  | 'working'
  | 'waiting_approval'
  | 'paused'
  | 'stopped'
  | 'deleted'
  | 'failed';

export interface ProfileReference {
  readonly id: string;
  readonly revision: number;
  readonly digest?: string;
}

export interface ConfiguredSandboxCapacity {
  readonly cpuMillis: number;
  readonly memoryBytes: number;
}

export interface SandboxProfileSettings {
  readonly agentProvider: 'codex';
  readonly modelAccess: 'chronoai_managed' | 'own_credentials';
  readonly nyxidServices: readonly string[];
}

export interface Failure {
  readonly class: string;
  readonly detail: string;
  readonly remediation?: string;
}

export interface ResourcePage<T, Cursor = string> {
  readonly items: readonly T[];
  readonly next?: Cursor;
}

export interface SandboxLifecycleEligibility {
  readonly allowed: boolean;
  readonly reason?: 'not_active' | 'not_paused' | 'checkpoint_in_progress' | 'work_in_progress';
}

export interface SandboxLifecycle {
  readonly result?: {
    readonly state: 'in_progress' | 'recoverable_pending' | 'failed';
    readonly code?: string;
    readonly operationId?: string;
    readonly startedAt?: string;
    readonly resultDueAt?: string;
    readonly continuing: boolean;
    readonly actions: readonly ('observe' | 'retire')[];
    readonly retryAfterSeconds?: number;
    readonly ownerSlotReleased?: boolean;
    readonly blockedRetirementStep?: string;
  };
  readonly pause: SandboxLifecycleEligibility;
  readonly revive: SandboxLifecycleEligibility;
  readonly interruption?: {
    readonly cause: 'out_of_memory' | 'runtime_restarted' | 'runtime_unavailable';
    readonly detectedAt: string;
    readonly memoryLimitMib: number;
  };
  readonly checkpoint?: {
    readonly phase: 'capture' | 'resume_writers';
    readonly startedAt: string;
    readonly epoch: number;
  };
  readonly backgroundSave?: {
    readonly phase:
      | 'capturing'
      | 'preparing_export'
      | 'exporting'
      | 'uploading'
      | 'finishing'
      | 'saved'
      | 'cancelled';
    readonly startedAt: string;
    readonly capturedAt?: string;
    readonly epoch: number;
  };
  readonly operation?: {
    readonly purpose: 'pause' | 'revive' | 'image_rotation';
    readonly phase: string;
    readonly startedAt: string;
  };
}

export interface SandboxSummary {
  readonly id: string;
  readonly owner?: string;
  readonly ownerName?: string;
  readonly name: string;
  /** The colour every Agent in this Sandbox wears (§28), as `#rrggbb`; absent until assigned. */
  readonly color?: string;
  readonly status: SandboxStatus;
  readonly lifecycle?: SandboxLifecycle;
  readonly runtimeUpdate?: SandboxRuntimeUpdate;
  readonly profile: ProfileReference;
  readonly alwaysOn?: boolean;
  readonly lastActivityAt: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly workspaceCount: number;
  readonly worktreeCount: number;
  readonly agentCount: number;
}

export interface SandboxRuntimeUpdate {
  readonly currentVersionConfirmed?: boolean;
  readonly imageOrigin?:
    'managed_agent' | 'managed_desktop' | 'managed_machine' | 'user_selected' | 'unknown';
  readonly currentVersion: string | null;
  readonly latestVersion: string;
  readonly updateAvailable: boolean;
  readonly state: 'up_to_date' | 'available' | 'queued' | 'updating' | 'failed';
  readonly canUpdate: boolean;
  readonly blockedReason?: string;
  readonly targetVersion?: string;
  readonly requestedAt?: string;
  readonly startedAt?: string;
  readonly completedAt?: string;
  readonly failure?: { readonly code: string; readonly message: string };
  readonly schedule: { readonly timezone: 'Asia/Singapore'; readonly localTime: '02:00' };
  readonly preservesHome: boolean;
}

export interface WorkspaceSummary {
  readonly id: string;
  readonly sandboxId: string;
  readonly name: string;
  readonly status: WorkspaceStatus;
  readonly profile: ProfileReference;
  readonly worktreeCount: number;
  readonly agentCount: number;
}

export interface SandboxDetail extends SandboxSummary {
  readonly agentProvider?: SandboxProfileSettings['agentProvider'];
  readonly modelAccess?: SandboxProfileSettings['modelAccess'];
  readonly nyxidServices?: readonly string[];
  readonly limits: {
    readonly maxWorkspaces: number;
    readonly maxAgentsPerWorkspace: number;
  };
  readonly runtimeVersion?: string;
  readonly failure?: Failure;
  readonly retiredAt?: string;
  readonly revision: number;
  readonly workspaces: readonly WorkspaceSummary[];
}

export type WorkspaceSource =
  | {
      readonly type: 'repository_snapshot';
      readonly url: string;
      readonly reference?: string;
      readonly source_commit?: string;
    }
  | { readonly type: 'git_repository'; readonly url: string; readonly reference?: string }
  | { readonly type: 'empty' };

export interface Worktree {
  readonly id: string;
  readonly workspaceId: string;
  readonly sandboxId: string;
  readonly name: string;
  readonly path: string;
  readonly branch: string;
  readonly baseCommit: string;
  readonly status: WorktreeStatus;
  readonly agentId?: string;
}

export interface AgentRuntimeMetadata {
  readonly workingDirectory?: string;
  readonly description?: string;
  readonly descriptionSource?: 'provider_title' | 'prompt_preview';
  /** Positive server proof that this preview came from automatic setup. */
  readonly descriptionIsSetupPreview?: boolean;
}
export interface AgentSummary {
  readonly taskAcceptance?: import('@cma/client').TaskAcceptance;
  readonly taskStartedAt?: string;
  readonly taskActivityAt?: string;
  readonly taskStatus?: import('@cma/client').AgentTaskStatus;
  readonly ownerTitle?: string;
  readonly runtimeMetadata?: AgentRuntimeMetadata;
  readonly id: string;
  readonly worktreeId: string;
  readonly workspaceId: string;
  readonly sandboxId: string;
  readonly status: AgentStatus;
  readonly profile: ProfileReference;
  readonly title?: string;
  readonly updatedAt: string;
}

export interface WorkspaceDetail extends WorkspaceSummary {
  readonly path: string;
  readonly source: WorkspaceSource;
  readonly resolvedCommit?: string;
  readonly failure?: Failure;
  readonly createdAt: string;
  readonly readyAt?: string;
  readonly removedAt?: string;
  readonly revision: number;
  readonly worktrees: readonly Worktree[];
  readonly agents: readonly AgentSummary[];
}

export interface AgentProfileSummary extends ProfileReference {
  readonly model?: string;
  readonly thinkingEffort: string;
  readonly permissionLevel: string;
  readonly skills: readonly {
    readonly source: string;
    readonly id: string;
    readonly version?: string;
  }[];
}

export type FirstMessageDelivery =
  | 'queued'
  | 'dispatch_fenced'
  | 'write_confirmed'
  | 'execution_observed'
  | 'completed'
  | 'failed'
  | 'recovery_required';

export interface FirstMessageState {
  readonly state: FirstMessageDelivery;
  readonly queuedAt: string;
  readonly completedAt?: string;
  readonly failedAt?: string;
  readonly purpose?: 'setup' | 'task';
  readonly turnSequence?: number;
}

export interface AgentDetail {
  readonly taskAcceptance?: import('@cma/client').TaskAcceptance;
  readonly ownerTitle?: string;
  readonly runtimeMetadata?: AgentRuntimeMetadata;
  readonly id: string;
  readonly status: AgentStatus;
  /** Current server proof of the lifecycle-owned bootstrap lineage. */
  readonly bootstrap?: {
    readonly operationId: string;
    readonly sandboxId: string;
    readonly workspaceId: string;
  };
  readonly profile: AgentProfileSummary;
  readonly worktree: Worktree;
  readonly workspace: {
    readonly id: string;
    readonly name: string;
    readonly path: string;
    readonly status: WorkspaceStatus;
    readonly profile: ProfileReference;
    readonly resolvedCommit?: string;
    readonly source?: WorkspaceSource;
  };
  readonly sandbox: {
    readonly id: string;
    readonly name: string;
    readonly status: SandboxStatus;
    readonly profile: ProfileReference;
  };
  readonly desktop?: boolean;
  readonly conformsTo: readonly string[];
  readonly reach: readonly string[];
  readonly title?: string;
  readonly pausedNotice?: string;
  readonly firstMessage?: FirstMessageState;
  readonly failure?: Failure;
  readonly stopReason?: string;
  readonly restartAvailable?: boolean;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly readyAt?: string;
  readonly stoppedAt?: string;
  readonly deletedAt?: string;
}

export interface AgentTranscriptMessage {
  readonly sequence: number;
  readonly role: 'user' | 'assistant';
  readonly content: string;
  readonly appendedAt: string;
}

export interface AgentTranscriptTurn {
  readonly retainedOutput?: { readonly available: boolean };
  readonly turnSequence: number;
  readonly startedAt: string;
  readonly ownerMessage: string;
  readonly agentMessages: readonly string[];
  readonly messages: readonly AgentTranscriptMessage[];
  readonly outcome: Readonly<Record<string, unknown>>;
}

export interface AgentTranscript {
  readonly agentId: string;
  readonly throughSeq: number;
  readonly turns: readonly AgentTranscriptTurn[];
}

export interface AgentEvent {
  readonly retainedOutput?: { readonly available: boolean };
  readonly seq: number;
  readonly appendedAt: string;
  readonly kind: Readonly<{ type: string } & Record<string, unknown>>;
}

export interface AgentUsage {
  readonly currentTurn?: {
    readonly turnSequence: number;
    readonly inputTokens: number;
    readonly outputTokens: number;
  };
  readonly agentId: string;
  readonly throughSeq: number;
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly contextUsed: number;
  readonly contextWindow?: number;
}

export interface SandboxEvent {
  readonly sandboxId: string;
  readonly seq: number;
  readonly appendedAt: string;
  readonly kind: Readonly<{ type: string } & Record<string, unknown>>;
}

export interface AgentLiveEvent {
  readonly 'cma:retainedOutput'?: { readonly available: boolean };
  readonly type: string;
  readonly runId: string;
  readonly threadId: string;
  readonly timestamp?: string;
  readonly cursor?: number;
  readonly [field: string]: unknown;
}

export interface AgentRunRequest {
  readonly forwardedProps?: { readonly answerContext: import('@cma/client').AnswerContext };
  readonly threadId: string;
  readonly runId: string;
  readonly messages?: readonly {
    readonly role: 'user';
    readonly content: string;
  }[];
  readonly resume?: readonly {
    readonly interruptId: string;
    readonly status: 'resolved';
    readonly payload: { readonly decision: 'allow' | 'deny' | 'abort' };
  }[];
}

export interface ProfileSelection {
  readonly id: string;
  readonly revision: number;
}

export interface CreateSandboxInput {
  readonly sandboxProfile: ProfileSelection;
  readonly name?: string;
  readonly trustConfirmed?: boolean;
  readonly commandsDigest?: string;
}

export interface CreateWorkspaceInput {
  readonly workspaceProfile: ProfileSelection;
  readonly trustConfirmed?: boolean;
  readonly commandsDigest?: string;
}

export type InlineWorkspaceSource =
  | { readonly type: 'git_repository'; readonly url: string; readonly reference?: string }
  | { readonly type: 'empty' };

export interface CreateInlineWorkspaceInput {
  readonly source: InlineWorkspaceSource;
  readonly name?: string;
}

export interface CreateAgentInput {
  readonly agentProfile: ProfileSelection;
  readonly title?: string;
  readonly firstMessage?: string;
}
