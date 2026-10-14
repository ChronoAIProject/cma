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
  readonly pause: SandboxLifecycleEligibility;
  readonly revive: SandboxLifecycleEligibility;
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
  readonly status: SandboxStatus;
  readonly lifecycle?: SandboxLifecycle;
  readonly profile: ProfileReference;
  readonly lastActivityAt: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly workspaceCount: number;
  readonly worktreeCount: number;
  readonly agentCount: number;
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

export interface AgentSummary {
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
}

export interface AgentDetail {
  readonly id: string;
  readonly status: AgentStatus;
  readonly profile: AgentProfileSummary;
  readonly worktree: Worktree;
  readonly workspace: {
    readonly id: string;
    readonly name: string;
    readonly path: string;
    readonly status: WorkspaceStatus;
    readonly profile: ProfileReference;
    readonly resolvedCommit?: string;
  };
  readonly sandbox: {
    readonly id: string;
    readonly name: string;
    readonly status: SandboxStatus;
    readonly profile: ProfileReference;
  };
  readonly conformsTo: readonly string[];
  readonly reach: readonly string[];
  readonly title?: string;
  readonly pausedNotice?: string;
  readonly firstMessage?: FirstMessageState;
  readonly failure?: Failure;
  readonly stopReason?: string;
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
  readonly seq: number;
  readonly appendedAt: string;
  readonly kind: Readonly<{ type: string } & Record<string, unknown>>;
}

export interface AgentUsage {
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
  readonly type: string;
  readonly runId: string;
  readonly threadId: string;
  readonly timestamp?: string;
  readonly cursor?: number;
  readonly [field: string]: unknown;
}

export interface AgentRunRequest {
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
