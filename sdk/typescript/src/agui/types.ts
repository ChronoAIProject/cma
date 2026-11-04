import type { OperationOptions } from '../http.js';
import type { AnswerContext } from '../answer-context.js';

export interface AgentConversationEvent {
  readonly type: string;
  readonly threadId: string;
  readonly runId: string;
  readonly timestamp?: string;
  readonly cursor?: number;
  readonly [field: string]: unknown;
}

export interface AgentRunRequest {
  readonly forwardedProps?: { readonly answerContext: AnswerContext };
  readonly threadId: string;
  readonly runId?: string;
  readonly messages?: readonly { readonly role: 'user'; readonly content: string }[];
  readonly resume?: readonly {
    readonly interruptId: string;
    readonly status: 'resolved';
    readonly payload: { readonly decision: 'allow' | 'deny' | 'abort' };
  }[];
}

export interface AgentStreamOptions extends OperationOptions {
  readonly onEvent: (event: AgentConversationEvent) => void | Promise<void>;
}

export interface AgentRunOptions extends AgentStreamOptions {
  /** Retain this key and the same input to recover an uncertain admitted run. */
  readonly idempotencyKey?: string;
}

export interface AgentReplayOptions extends AgentStreamOptions {
  readonly after?: number;
  /** False reads one captured durable history and closes without reconnecting. */
  readonly follow?: boolean;
  /** Maximum consecutive connections without a newly acknowledged durable event. */
  readonly maxReconnects?: number;
}
