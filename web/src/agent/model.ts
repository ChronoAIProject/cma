import type { AgentStatus, AgentUsage, AgentRuntimeMetadata } from '../api/types';
import type { SessionCommand } from './slashCommands';
import type { FailureMessage } from './failureMessage';
import type { ExternalApprovalProjection } from '../api-client/agent-events-v2/external-approval';
import type { ExternalApprovalRequest, ExternalApprovalReviewPolicy } from './externalApproval';

export interface ConversationMessage {
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  /** Stable DOM identity carried from an ephemeral live row to durable history. */
  readonly renderKey?: string;
  readonly runId?: string;
  readonly role: 'user' | 'assistant';
  readonly content: string;
  readonly status: 'streaming' | 'complete' | 'error';
  readonly at?: string;
  readonly progressAt?: string;
  readonly liveOnly?: boolean;
  readonly finalAnswer?: boolean;
  /** Render-only retained text; never a durable message or a successful answer. */
  readonly retainedPartial?: {
    readonly loading: boolean;
    readonly error?: string;
    readonly retry: () => void;
  };
  /** Exact durable Responses command that owns this message, when available. */
  readonly commandId?: string;
  readonly responseId?: string;
}

export interface ToolCall {
  readonly startedAt?: string;
  readonly finishedAt?: string;
  readonly progressAt?: string;
  readonly output?: { readonly text: string; readonly truncated: boolean };
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly runId?: string;
  readonly title: string;
  readonly kind?: string;
  readonly status: string;
  readonly detail?: string;
}

export interface ReasoningBlock {
  readonly startedAt?: string;
  readonly progressAt?: string;
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly runId?: string;
  readonly content: string;
  readonly status: 'streaming' | 'complete';
}

export interface PlanEntry {
  readonly title: string;
  readonly status: string;
}

/** One turn's latest reported plan; a provider restates the whole plan on every change. */
export interface PlanSnapshot {
  readonly progressAt?: string;
  readonly incomplete?: boolean;
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly runId?: string;
  readonly entries: readonly PlanEntry[];
}

export interface NativeApprovalRequest {
  readonly source?: 'native';
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly reason: string;
  readonly message?: string;
  readonly decision?: string;
}

export type ApprovalRequest = NativeApprovalRequest | ExternalApprovalRequest;

export interface RecoveryNotice {
  readonly retainedOutput?: { readonly available: boolean };
  readonly order?: number;
  readonly sequence?: number;
  readonly runId?: string;
  readonly afterSequence?: number;
  readonly id: string;
  readonly title: string;
  readonly detail: string;
  readonly nextStep?: string;
  readonly technicalDetails?: FailureMessage['technicalDetails'];
  readonly url?: string;
  readonly code?: string;
  /** Live recovery clears only after the corresponding observation is restored. */
  readonly clearsOn?: 'live-update' | 'recorded-turn';
}

export interface ConversationState {
  /** Runs whose live tail is no longer contiguous after a reconnect. */
  readonly liveGapRuns?: ReadonlySet<string>;
  readonly turnOutcomes?: Readonly<Record<string, 'completed' | 'failed' | 'interrupted'>>;
  readonly agentCursor?: string;
  readonly agentIncarnation?: string;
  readonly responseBinding?: {
    readonly responseId: string;
    readonly commandId: string;
    readonly turnSequence: number;
  };
  readonly messages: readonly ConversationMessage[];
  readonly toolCalls: readonly ToolCall[];
  readonly reasoning: readonly ReasoningBlock[];
  readonly approvals: readonly ApprovalRequest[];
  readonly externalApprovalProjection?: ExternalApprovalProjection;
  readonly externalApprovalReviewPolicy?: ExternalApprovalReviewPolicy;
  readonly plans: readonly PlanSnapshot[];
  readonly recovery: readonly RecoveryNotice[];
  readonly usage?: AgentUsage;
  readonly turnUsage?: Readonly<Record<string, NonNullable<AgentUsage['currentTurn']>>>;
  readonly turnTiming?: Readonly<
    Record<string, { readonly startedAt: string; readonly finishedAt?: string }>
  >;
  /** Ephemeral visible text already covered by the latest provider usage report. */
  readonly visibleUsageAnchor?: {
    readonly turnSequence: number;
    readonly inputTokens: number;
    readonly outputTokens: number;
    readonly parts: readonly { readonly id: string; readonly text: string }[];
  };
  readonly runtimeMetadata?: AgentRuntimeMetadata;
  readonly ownerTitle?: string;
  readonly title?: string;
  readonly availableCommands?: readonly SessionCommand[];
  readonly activeRunId?: string;
  readonly latestTurnId?: string;
  /** Recorded turn start, independent of route mounts and stream reconnects. */
  readonly activeRunStartedAt?: string;
  readonly agentStatus?: AgentStatus;
  readonly agentStatusAt?: string;
  readonly livePausedNotice?: string | null;
  readonly cursor: number;
}
