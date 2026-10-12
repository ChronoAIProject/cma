import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { input, read, segment } from '../resources.js';
import {
  OutboundMessageSchema,
  IntentReceiptSchema,
  TypingReceiptSchema,
  ContextResponseSchema,
  ReplyTargetResponseSchema,
  type OutboundMessage,
} from './types.js';

/** Tools require the channel agent key; opaque references must not be logged. */
export class CmaegTools {
  constructor(private readonly config: ResolvedConfig) {}

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

  replyToEvent(eventRef: string, message: OutboundMessage, options: OperationOptions = {}) {
    return read(this.config, `/v1/events/${segment(eventRef)}/replies`, IntentReceiptSchema, {
      ...options,
      method: 'POST',
      body: input(OutboundMessageSchema, message),
    });
  }

  updateMessage(messageRef: string, message: OutboundMessage, options: OperationOptions = {}) {
    return read(this.config, `/v1/messages/${segment(messageRef)}/update`, IntentReceiptSchema, {
      ...options,
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

  readEventContext(eventRef: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/events/${segment(eventRef)}/context`,
      ContextResponseSchema,
      options,
    );
  }
}
