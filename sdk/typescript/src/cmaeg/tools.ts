import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { input, read, segment } from '../resources.js';
import {
  FormPreparationReceiptSchema,
  OutboundMessageSchema,
  IntentReceiptSchema,
  TypingReceiptSchema,
  ContextResponseSchema,
  ReplyTargetResponseSchema,
  type OutboundMessage,
  MessageEffectReceiptSchema,
  MessageEffectsCapabilitiesSchema,
  MessageReactionRequestSchema,
  type MessageReactionRequest,
  ThreadListSchema,
  ThreadReceiptSchema,
  CreateThreadRequestSchema,
  ManageThreadRequestSchema,
  type ThreadListView,
  type CreateThreadRequest,
  type ManageThreadRequest,
} from './types.js';

/** Tools require the channel agent key; opaque references must not be logged. */
export class CmaegTools {
  constructor(private readonly config: ResolvedConfig) {}

  /** Bounded original-event scope. No private sibling enumeration or pagination. */
  listEventThreads(
    eventRef: string,
    view: ThreadListView = 'active',
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/threads?view=${segment(view)}`,
      ThreadListSchema,
      options,
    );
  }
  /** Create from the exact original message. Never retry unknown outcomes under another key. */
  createEventThread(
    eventRef: string,
    idempotencyKey: string,
    request: CreateThreadRequest,
    options: OperationOptions = {},
  ) {
    return read(this.config, `/v1/events/${segment(eventRef)}/threads`, ThreadReceiptSchema, {
      ...options,
      method: 'POST',
      headers: { 'Idempotency-Key': idempotencyKey },
      body: input(CreateThreadRequestSchema, request),
    });
  }
  /** Archive/reopen an exact thread or join/leave as the current bot. */
  manageThread(
    idempotencyKey: string,
    request: ManageThreadRequest,
    options: OperationOptions = {},
  ) {
    return read(this.config, '/v1/thread-operations', ThreadReceiptSchema, {
      ...options,
      method: 'POST',
      headers: { 'Idempotency-Key': idempotencyKey },
      body: input(ManageThreadRequestSchema, request),
    });
  }

  readMessageCapabilities(messageRef: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/messages/${segment(messageRef)}/capabilities`,
      MessageEffectsCapabilitiesSchema,
      options,
    );
  }
  deleteMessage(messageRef: string, idempotencyKey: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/messages/${segment(messageRef)}/delete`,
      MessageEffectReceiptSchema,
      {
        ...options,
        method: 'POST',
        headers: { 'Idempotency-Key': idempotencyKey },
        body: {},
      },
    );
  }
  setMessageReaction(
    messageRef: string,
    idempotencyKey: string,
    request: MessageReactionRequest,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/v1/messages/${segment(messageRef)}/reaction`,
      MessageEffectReceiptSchema,
      {
        ...options,
        method: 'POST',
        headers: { 'Idempotency-Key': idempotencyKey },
        body: input(MessageReactionRequestSchema, request),
      },
    );
  }

  createReplyTarget(eventRef: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/reply-target`,
      ReplyTargetResponseSchema,
      {
        ...options,
        method: 'POST',
        body: {},
      },
    );
  }

  replyToTarget(
    targetRef: string,
    idempotencyKey: string,
    message: OutboundMessage,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/v1/reply-targets/${segment(targetRef)}/messages`,
      IntentReceiptSchema,
      {
        ...options,
        method: 'POST',
        headers: { 'Idempotency-Key': idempotencyKey },
        body: input(OutboundMessageSchema, message),
      },
    );
  }

  /** Admit preparation only. Reuse the exact message and key after any timeout. */
  prepareFormReply(
    eventRef: string,
    message: OutboundMessage,
    idempotencyKey: string,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/form-preparation`,
      FormPreparationReceiptSchema,
      {
        ...options,
        method: 'POST',
        headers: { 'Idempotency-Key': idempotencyKey },
        body: input(OutboundMessageSchema, message),
      },
    );
  }
  /** Read only; Ready does not mean that a customer message was sent. */
  readFormPreparation(eventRef: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/form-preparation`,
      FormPreparationReceiptSchema,
      options,
    );
  }
  replyToEvent(
    eventRef: string,
    message: OutboundMessage,
    options: OperationOptions & { idempotencyKey?: string; formPreparationRef?: string } = {},
  ) {
    const { idempotencyKey, formPreparationRef, ...operationOptions } = options;
    return read(this.config, `/v1/events/${segment(eventRef)}/replies`, IntentReceiptSchema, {
      ...operationOptions,
      headers: {
        ...(idempotencyKey === undefined ? {} : { 'Idempotency-Key': idempotencyKey }),
        ...(formPreparationRef === undefined ? {} : { 'Form-Preparation-Ref': formPreparationRef }),
      },
      method: 'POST',
      body: input(OutboundMessageSchema, message),
    });
  }

  /** Native media updates require a stable caller key; retain the returned message_ref. */
  updateMessage(
    messageRef: string,
    message: OutboundMessage,
    options: OperationOptions & { idempotencyKey?: string } = {},
  ) {
    const { idempotencyKey, ...operationOptions } = options;
    return read(this.config, `/v1/messages/${segment(messageRef)}/update`, IntentReceiptSchema, {
      ...operationOptions,
      ...(idempotencyKey === undefined ? {} : { headers: { 'Idempotency-Key': idempotencyKey } }),
      method: 'POST',
      body: input(OutboundMessageSchema, message),
    });
  }

  sendTyping(eventRef: string, options: OperationOptions = {}) {
    return read(this.config, `/v1/events/${segment(eventRef)}/typing`, TypingReceiptSchema, {
      ...options,
      method: 'POST',
    });
  }

  readEventContext(
    eventRef: string,
    options: OperationOptions & { includeThreadHistory?: boolean } = {},
  ) {
    const { includeThreadHistory, ...operationOptions } = options;
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/context${includeThreadHistory ? '?include_thread_history=true' : ''}`,
      ContextResponseSchema,
      operationOptions,
    );
  }
}
