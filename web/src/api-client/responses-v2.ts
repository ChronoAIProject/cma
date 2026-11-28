import { readRetainedOutput, type RetainedOutputOptions } from './agents/retained-output.js';
import { readOutputPage, type V2OutputPageOptions } from './responses-v2/output-page.js';
export type { V2OutputPage, V2OutputPageOptions } from './responses-v2/output-page.js';
import { v2EventContext } from './responses-v2/event-context.js';
import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import type { OperationOptions } from './http.js';
import { identity, input, query, read, segment } from './resources.js';
import { eventStream } from './streaming.js';
import { commandStreamInput } from './command-stream-input.js';
import { conversationId, v2Request } from './responses-v2/requests.js';
import { v2ResponseFrames } from './responses-v2/stream.js';
import { submitResponseStream, type V2RecoveryNotice } from './responses-v2/recovery.js';
export type { V2RecoveryNotice } from './responses-v2/recovery.js';
import * as controls from './responses-v2/controls.js';
import * as interruption from './responses-v2/interruption.js';
import type { V2ResponseInterruptionRequest } from './responses-v2/interruption.js';
import * as queuedCancellation from './responses-v2/queued-cancellation.js';
import * as queuedInput from './responses-v2/queued-input.js';
import type { V2QueuedInputRequest } from './responses-v2/queued-input-types.js';
import type { V2QueuedCancellationRequest } from './responses-v2/queued-cancellation-types.js';
import { v2CommandStatus, type V2ResponseControlRequest } from './responses-v2/controls-types.js';
import type { CommandOptions } from './resources.js';
import {
  v2Capabilities,
  v2Conversation,
  v2ConversationItemPage,
  v2ResponseObject,
  type CreateV2ResponseRequest,
  type CreateV2AgentResponseRequest,
  type V2ResponseEvent,
  type V2ResponseObject,
  type V2ResponseReceipt,
} from './responses-v2/types.js';
export type * from './responses-v2/types.js';
export type * from './responses-v2/controls-types.js';
export type {
  V2ResponseInterruptionRequest,
  V2ResponseInterruption,
} from './responses-v2/interruption.js';
export type * from './responses-v2/queued-cancellation-types.js';
export type * from './responses-v2/queued-input-types.js';
export { commandIsTerminal } from './responses-v2/controls-types.js';

export interface V2ResponseStreamOptions extends OperationOptions {
  /** Persist this key before sending. Retry explicitly with the same body/key after a lost reply. */
  readonly idempotencyKey?: string;
  readonly onReceipt?: (receipt: V2ResponseReceipt) => void | Promise<void>;
  /** Observe the correlation and cursor of a bounded read-only recovery attempt. */
  readonly onRecovery?: (notice: V2RecoveryNotice) => void | Promise<void>;
  readonly onEvent: (event: V2ResponseEvent) => void | Promise<void>;
}
export interface V2ReplayOptions extends Omit<V2ResponseStreamOptions, 'idempotencyKey'> {
  /** Last acknowledged durable sequence. Omit to replay from the beginning. */
  readonly startingAfter?: number;
}
export interface V2ConversationItemsOptions extends OperationOptions {
  readonly after?: string;
  readonly before?: string;
  readonly order?: 'asc' | 'desc';
  readonly limit?: number;
}
/** Versioned streaming Responses subset using durable Agent command admission. */
export class ResponsesV2 {
  constructor(private readonly config: ResolvedConfig) {}
  /** Discover and page retained partial text without changing the original failed outcome. */
  retainedOutput(id: string, options: RetainedOutputOptions = {}) {
    return readRetainedOutput(
      this.config,
      `/api/v2/responses/${segment(id)}/retained-output`,
      options,
    );
  }
  /** Observe an exact bound turn. Runtime support is an observation, not caller permission. */
  observeControls(agentId: string, responseId: string, options: OperationOptions = {}) {
    return controls.observeControls(this.config, agentId, responseId, options);
  }
  /** Submit once. Recover uncertainty with the identical body/key; never retarget on retry. */
  submitControl(
    agentId: string,
    responseId: string,
    body: V2ResponseControlRequest,
    options: CommandOptions,
  ) {
    return controls.submitControl(this.config, agentId, responseId, body, options);
  }
  /** Cancel the exact active turn and admit a distinct next response atomically. */
  interruptWithInput(
    agentId: string,
    responseId: string,
    body: V2ResponseInterruptionRequest,
    options: CommandOptions,
  ) {
    return interruption.interruptWithInput(this.config, agentId, responseId, body, options);
  }
  /** One safe command-status poll. Completed control means delivery; inspect the target outcome separately. */
  readCommand(commandId: string, options: OperationOptions = {}) {
    return controls.readCommand(this.config, commandId, options);
  }
  /** Observe eligibility to cancel this exact source before it binds a turn. */
  observeQueuedCancellation(agentId: string, responseId: string, options: OperationOptions = {}) {
    return queuedCancellation.observeQueuedCancellation(this.config, agentId, responseId, options);
  }
  /** Cancel once. Recover uncertainty with the identical source, revision, route and key. */
  cancelQueuedResponse(
    agentId: string,
    responseId: string,
    body: V2QueuedCancellationRequest,
    options: CommandOptions,
  ) {
    return queuedCancellation.cancelQueuedResponse(this.config, agentId, responseId, body, options);
  }
  /** Ordered authoritative input queue, bounded by the server admission limit of 64. */
  listQueuedInputs(agentId: string, options: OperationOptions = {}) {
    return queuedInput.listQueuedInputs(this.config, agentId, options);
  }
  /** Replace pending text in place. Retain the exact body/key until the outcome is known. */
  editQueuedInput(
    agentId: string,
    responseId: string,
    body: V2QueuedInputRequest,
    options: CommandOptions,
  ) {
    return queuedInput.editQueuedInput(this.config, agentId, responseId, body, options);
  }
  readCapabilities(options: OperationOptions = {}) {
    return read(this.config, '/api/v2', v2Capabilities, options);
  }
  read(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v2/responses/${segment(id)}`,
      identity(v2ResponseObject, id),
      options,
    );
  }
  /** Read one bounded output page using only this response's returned cursor. */
  readOutputPage(id: string, options: V2OutputPageOptions = {}) {
    return readOutputPage(this.config, id, options);
  }
  readEventContext(
    binding: string,
    conversation: string,
    event: string,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/api/v1/cmaeg/bindings/${segment(binding)}/conversations/${segment(conversation)}/events/${segment(event)}`,
      (value) => {
        const context = v2EventContext(value);
        if (context.conversation_id !== conversation || context.activity.event_id !== event)
          throw new ProtocolError('Event context response identity mismatch');
        return context;
      },
      options,
    );
  }
  readConversation(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v2/conversations/${segment(id)}`,
      id.startsWith('agt_') ? identity(v2Conversation, id) : v2Conversation,
      options,
    );
  }
  async listConversationItems(id: string, options: V2ConversationItemsOptions = {}) {
    if (
      options.limit !== undefined &&
      (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 100)
    )
      throw new ValidationError('limit must be between 1 and 100');
    if (options.order !== undefined && !['asc', 'desc'].includes(options.order))
      throw new ValidationError('order must be asc or desc');
    const page = await read(
      this.config,
      query(`/api/v2/conversations/${segment(id)}/items`, {
        after: options.after,
        before: options.before,
        order: options.order,
        limit: options.limit,
      }),
      v2ConversationItemPage,
      options,
    );
    if (
      (page.data[0]?.id ?? null) !== page.first_id ||
      (page.data.at(-1)?.id ?? null) !== page.last_id ||
      new Set(page.data.map((item) => item.id)).size !== page.data.length
    )
      throw new ProtocolError('Conversation item cursors or identities are inconsistent');
    return page;
  }
  create(
    body: CreateV2ResponseRequest,
    options: V2ResponseStreamOptions,
  ): Promise<V2ResponseObject> {
    return this.submit('/api/v2/responses', body, options);
  }
  createForAgent(
    id: string,
    body: CreateV2AgentResponseRequest,
    options: V2ResponseStreamOptions,
  ): Promise<V2ResponseObject> {
    return this.submit(`/api/v2/agents/${segment(id)}/responses`, body, options, id);
  }
  /** Look up the exact admitted input without submitting it. A miss is indeterminate. */
  findAgentInputReceipt(
    id: string,
    body: CreateV2AgentResponseRequest,
    options: CommandOptions & { readonly requireJson?: boolean },
  ) {
    input((value) => v2Request(value, id), body);
    const headers = responseKeyHeaders(options.idempotencyKey);
    if (headers === undefined) throw new ValidationError('Supply the original idempotency key');
    return read(
      this.config,
      `/api/v2/agents/${segment(id)}/responses/receipt`,
      (value) => {
        if (value === null) return null;
        const status = v2CommandStatus(value);
        if (status.receipt.agent_id !== id || status.receipt.response_id === null) {
          throw new ProtocolError('input receipt does not match the selected Agent');
        }
        return status;
      },
      { ...options, headers, body, method: 'POST', cache: 'no-store' },
    );
  }
  /** Reconnect only on explicit caller request; never resubmit input on disconnect. */
  async replay(id: string, options: V2ReplayOptions): Promise<void> {
    if (
      options.startingAfter !== undefined &&
      (!Number.isSafeInteger(options.startingAfter) ||
        options.startingAfter < 0 ||
        options.startingAfter >= Number.MAX_SAFE_INTEGER)
    )
      throw new ValidationError('startingAfter must be a nonnegative safe cursor');
    const stream = eventStream(
      this.config,
      query(`/api/v2/responses/${segment(id)}/events`, { starting_after: options.startingAfter }),
      options,
      (response, signal) =>
        v2ResponseFrames(response, signal, {
          responseId: id,
          startingAfter: options.startingAfter,
          onReceipt: options.onReceipt,
        }),
      'Responses v2',
    );
    for await (const event of stream) await acknowledge(options, event);
  }
  private async submit(
    path: string,
    body: CreateV2AgentResponseRequest,
    options: V2ResponseStreamOptions,
    agent?: string,
  ): Promise<V2ResponseObject> {
    // JSON serialization can omit fields that the original request must reject.
    input((value) => v2Request(value, agent), body);
    const { body: snapshot, headers } = commandStreamInput(body, options.idempotencyKey);
    input((value) => v2Request(value, agent), snapshot);
    const conversation = agent ?? conversationId(snapshot.conversation);
    return submitResponseStream(
      this.config,
      path,
      snapshot,
      headers,
      options,
      conversation?.startsWith('agt_') ? conversation : undefined,
    );
  }
}

function responseKeyHeaders(key: string | undefined) {
  if (key === undefined) return undefined;
  if (
    !key.length ||
    new TextEncoder().encode(key).length > 256 ||
    [...key].some(
      (char) => char.charCodeAt(0) < 32 || (char.charCodeAt(0) >= 127 && char.charCodeAt(0) <= 159),
    )
  ) {
    throw new ValidationError(
      'idempotencyKey must contain 1 to 256 bytes without control characters',
    );
  }
  return { 'Idempotency-Key': key };
}

async function acknowledge(options: V2ReplayOptions, event: V2ResponseEvent) {
  try {
    await options.onEvent(event);
  } catch (cause) {
    throw new ProtocolError('Responses consumer did not acknowledge a frame', { cause });
  }
}
