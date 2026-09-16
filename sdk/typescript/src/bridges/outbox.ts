import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import {
  command,
  input,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
} from '../resources.js';
import { positiveInteger } from '../schema.js';
import { BridgeInvokes } from './invoke.js';
import * as schema from './outbox-schema.js';

function path(id: string) {
  return `/api/v1/bridges/${segment(id)}`;
}
/** Read delivery receipts and explicitly recover under current owner/admin authority. */
export class BridgeOutbox extends BridgeInvokes {
  listDeliveries(id: string, options: PageOptions = {}) {
    if (options.limit !== undefined && input(positiveInteger, options.limit) > 100)
      throw new ValidationError('delivery limit exceeds 100');
    if (options.cursor !== undefined) input(schema.itemId, options.cursor);
    return read(
      this.config,
      query(`${path(id)}/deliveries`, { cursor: options.cursor, limit: options.limit }),
      (value) => {
        const result = schema.page(value);
        if (result.items.length > (options.limit ?? 50))
          throw new ProtocolError('delivery page exceeds requested limit');
        let previous = options.cursor;
        for (const item of result.items) {
          if (item.bridge_id !== id || (previous !== undefined && item.id <= previous))
            throw new ProtocolError('delivery page changed Bridge or failed to advance');
          previous = item.id;
        }
        if (
          result.next_cursor !== null &&
          (result.items.length === 0 || result.next_cursor !== previous)
        )
          throw new ProtocolError('delivery continuation does not match its page');
        return result;
      },
      options,
    );
  }
  readDelivery(id: string, itemId: string, options: OperationOptions = {}) {
    input(schema.itemId, itemId);
    return read(
      this.config,
      `${path(id)}/deliveries/${segment(itemId)}`,
      (value) => {
        const item = schema.item(value);
        if (item.id !== itemId || item.bridge_id !== id)
          throw new ProtocolError('delivery does not match requested Bridge and item');
        return item;
      },
      options,
    );
  }
  /** Preserve this exact request/key after cancellation or response loss. Unknown retry requires explicit duplicate-risk acceptance. */
  recoverDelivery(id: string, body: schema.BridgeDeliveryRecoveryRequest, options: CommandOptions) {
    const request = schema.recoveryRequest(input(schema.recoveryRequest, body));
    return read(
      this.config,
      `${path(id)}/delivery-recovery`,
      (value) => {
        const receipt = schema.recoveryReceipt(value);
        const revision = receipt.result.revision;
        if (
          receipt.bridge_id !== id ||
          receipt.result.id !== request.item_id ||
          receipt.action.kind !== request.action.kind ||
          (receipt.action.kind === 'retry' &&
            request.action.kind === 'retry' &&
            receipt.action.accept_duplicate_risk !== request.action.accept_duplicate_risk) ||
          (revision !== request.expected_revision + 1 &&
            !(request.action.kind === 'reconcile' && revision === request.expected_revision)) ||
          (request.action.kind === 'retry' && receipt.result.status !== 'pending') ||
          (request.action.kind === 'abandon' && receipt.result.status !== 'abandoned')
        )
          throw new ProtocolError('delivery recovery receipt does not match its command');
        return receipt;
      },
      { ...command(options), method: 'POST', body: request },
    );
  }
}
