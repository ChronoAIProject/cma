import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { requestVoid, type OperationOptions } from './http.js';
import { identity, input, query, read, segment } from './resources.js';
import { eventStream } from './streaming.js';
import { createResponseRequest } from './responses/requests.js';
import { responseFrames } from './responses/stream.js';
import type { ResponseEvent } from './responses/events.js';
import {
  conversation,
  conversationItemPage,
  createConversationRequest,
  responseObject,
  type CreateConversationRequest,
  type CreateResponseRequest,
  type ResponseObject,
} from './responses/types.js';

export type * from './responses/types.js';
export type { ResponseEvent } from './responses/events.js';
export interface ResponseStreamOptions extends OperationOptions {
  readonly onEvent: (event: ResponseEvent) => void | Promise<void>;
}
export interface ConversationItemsOptions extends OperationOptions {
  readonly after?: string;
  readonly limit?: number;
}

/** Stateful Responses subset over the same owned Agent conversation as AG-UI. */
export class Responses {
  constructor(private readonly config: ResolvedConfig) {}

  createConversation(body: CreateConversationRequest = {}, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/conversations', conversation, {
      ...options,
      method: 'POST',
      body: input(createConversationRequest, body),
    });
  }
  readConversation(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/conversations/${segment(id)}`,
      identity(conversation, id),
      options,
    );
  }
  async listConversationItems(id: string, options: ConversationItemsOptions = {}) {
    if (
      options.limit !== undefined &&
      (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 100)
    )
      throw new ValidationError('limit must be between 1 and 100');
    const page = await read(
      this.config,
      query(`/api/v1/conversations/${segment(id)}/items`, {
        after: options.after,
        limit: options.limit,
      }),
      conversationItemPage,
      options,
    );
    if (
      (page.data[0]?.id ?? null) !== page.first_id ||
      (page.data.at(-1)?.id ?? null) !== page.last_id
    )
      throw new ProtocolError('Conversation item cursors do not match the returned page');
    return page;
  }

  /** Submit once; disconnects never resubmit input. The returned object is the terminal frame. */
  async create(
    body: CreateResponseRequest,
    options: ResponseStreamOptions,
  ): Promise<ResponseObject> {
    input(createResponseRequest, body);
    const stream = eventStream(
      this.config,
      '/api/v1/responses',
      {
        ...options,
        timeoutMs: options.timeoutMs ?? 605_000,
      },
      (response, signal) => responseFrames(response, signal, body.conversation),
      'Responses',
      body,
    );
    let outcome: ResponseObject | undefined;
    for await (const event of stream) {
      try {
        await options.onEvent(event);
      } catch (cause) {
        throw new ProtocolError('Responses consumer did not acknowledge a frame', { cause });
      }
      if (
        'response' in event &&
        ['completed', 'failed', 'incomplete'].includes(event.response.status)
      ) {
        outcome = event.response;
        break;
      }
    }
    if (outcome === undefined)
      throw new ProtocolError('Responses stream returned no terminal resource');
    return outcome;
  }
  read(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/responses/${segment(id)}`,
      identity(responseObject, id),
      options,
    );
  }
  /** Unsupported by this deployment; preserves its named problem as ApiError. */
  async delete(id: string, options: OperationOptions = {}): Promise<never> {
    await requestVoid(this.config, `/api/v1/responses/${segment(id)}`, {
      ...options,
      method: 'DELETE',
    });
    throw new ProtocolError('Deleting an individual durable response is unsupported');
  }
  /** Unsupported by this deployment; use the Agent interrupt operation for active work. */
  async cancel(id: string, options: OperationOptions = {}): Promise<never> {
    await requestVoid(this.config, `/api/v1/responses/${segment(id)}/cancel`, {
      ...options,
      method: 'POST',
    });
    throw new ProtocolError('Background response cancellation is unsupported');
  }
}
