import type { AgentStatus, AgentUsage } from '../api/types';

export interface ConversationMessage {
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly runId?: string;
  readonly role: 'user' | 'assistant';
  readonly content: string;
  readonly status: 'streaming' | 'complete' | 'error';
  readonly at?: string;
  readonly liveOnly?: boolean;
  /** Exact durable Responses command that owns this message, when available. */
  readonly commandId?: string;
}

export interface ToolCall {
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
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly runId?: string;
  readonly content: string;
  readonly status: 'streaming' | 'complete';
}

export interface ApprovalRequest {
  readonly order?: number;
  readonly sequence?: number;
  readonly id: string;
  readonly reason: string;
  readonly message?: string;
  readonly decision?: string;
}

export interface RecoveryNotice {
  readonly order?: number;
  readonly sequence?: number;
  readonly runId?: string;
  readonly afterSequence?: number;
  readonly id: string;
  readonly title: string;
  readonly detail: string;
  readonly url?: string;
  readonly code?: string;
}

export interface ConversationState {
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
  readonly recovery: readonly RecoveryNotice[];
  readonly usage?: AgentUsage;
  readonly activeRunId?: string;
  /** Recorded turn start, independent of route mounts and stream reconnects. */
  readonly activeRunStartedAt?: string;
  readonly agentStatus?: AgentStatus;
  readonly agentStatusAt?: string;
  readonly livePausedNotice?: string | null;
  readonly cursor: number;
}
