import type { ResolvedConfig } from '../config.js';
import { CmaError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { input, read, segment } from '../resources.js';
import {
  OutboundMessageSchema,
  OutboundReceiptSchema,
  PrepareOutboundMessageSchema,
  type OutboundMessage,
  type PrepareOutboundMessage,
} from './types.js';

export interface PrepareOutboundOptions extends OperationOptions {
  /** Reuse only for this exact logical message; retained for seven days. */
  readonly idempotencyKey: string;
}
/** Inspect this delivery using GET. Never automatically resend after this error. */
export class OutboundCommitUncertainError extends CmaError {
  readonly isRetryable = false;
  constructor(
    readonly deliveryId: string,
    cause: unknown,
  ) {
    super(
      'OutboundCommitUncertainError',
      'Read the prepared delivery status before taking further action.',
      { cause },
    );
  }
}
export class CmaegOutbound {
  constructor(private readonly config: ResolvedConfig) {}
  prepareOutboundMessage(
    channel: string,
    body: PrepareOutboundMessage,
    options: PrepareOutboundOptions,
  ) {
    if (!/^[\x21-\x7e]{1,128}$/.test(options.idempotencyKey))
      throw new ValidationError('idempotencyKey must contain 1–128 visible ASCII bytes');
    return read(this.config, this.path(channel), OutboundReceiptSchema, {
      ...options,
      method: 'POST',
      expectedStatus: 201,
      headers: { 'Idempotency-Key': options.idempotencyKey },
      body: input(PrepareOutboundMessageSchema, body),
    });
  }
  getOutboundMessage(channel: string, delivery: string, options: OperationOptions = {}) {
    return read(this.config, this.path(channel, delivery), OutboundReceiptSchema, options);
  }
  cancelOutboundMessage(channel: string, delivery: string, options: OperationOptions = {}) {
    return read(this.config, this.path(channel, delivery), OutboundReceiptSchema, {
      ...options,
      method: 'DELETE',
    });
  }
  commitOutboundMessage(
    channel: string,
    delivery: string,
    body: OutboundMessage,
    options: OperationOptions = {},
  ) {
    return read(this.config, `${this.path(channel, delivery)}/send`, OutboundReceiptSchema, {
      ...options,
      method: 'POST',
      body: input(OutboundMessageSchema, body),
    });
  }
  async sendOutboundMessage(
    channel: string,
    body: PrepareOutboundMessage,
    options: PrepareOutboundOptions,
  ) {
    const snapshot = PrepareOutboundMessageSchema(body);
    const prepared = await this.prepareOutboundMessage(channel, snapshot, options);
    if (prepared.status !== 'prepared') return prepared;
    try {
      return await this.commitOutboundMessage(
        channel,
        prepared.delivery_id,
        snapshot.message,
        options,
      );
    } catch (cause) {
      throw new OutboundCommitUncertainError(prepared.delivery_id, cause);
    }
  }
  private path(channel: string, delivery?: string) {
    const base = `/v1/channels/${segment(channel)}/outbound-messages`;
    return delivery === undefined ? base : `${base}/${segment(delivery)}`;
  }
}
