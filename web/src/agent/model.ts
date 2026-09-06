import type { AgentStatus, AgentUsage } from '../api/types';

export interface ConversationMessage {
  readonly order?: number;
  readonly id: string;
  readonly runId?: string;
  readonly role: 'user' | 'assistant';
  readonly content: string;
  readonly status: 'streaming' | 'complete' | 'error';
  readonly at?: string;
  readonly liveOnly?: boolean;
}

export interface ToolCall {
  readonly order?: number;
  readonly id: string;
  readonly runId?: string;
  readonly title: string;
  readonly kind?: string;
  readonly status: string;
  readonly detail?: string;
}

export interface ReasoningBlock {
  readonly order?: number;
  readonly id: string;
  readonly runId?: string;
  readonly content: string;
  readonly status: 'streaming' | 'complete';
}

export interface ApprovalRequest {
  readonly order?: number;
  readonly id: string;
  readonly reason: string;
  readonly message?: string;
  readonly decision?: string;
}

export interface RecoveryNotice {
  readonly order?: number;
  readonly id: string;
  readonly title: string;
  readonly detail: string;
  readonly url?: string;
  readonly code?: string;
}

export interface ConversationState {
  readonly messages: readonly ConversationMessage[];
  readonly toolCalls: readonly ToolCall[];
  readonly reasoning: readonly ReasoningBlock[];
  readonly approvals: readonly ApprovalRequest[];
  readonly recovery: readonly RecoveryNotice[];
  readonly usage?: AgentUsage;
  readonly activeRunId?: string;
  readonly agentStatus?: AgentStatus;
  readonly livePausedNotice?: string | null;
  readonly cursor: number;
}
