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
import { boundedId } from './invoke-schema.js';
import { BridgeOutbox } from './outbox.js';
import * as schema from './cleanup-schema.js';

function path(id: string) {
  input(boundedId, id);
  return `/api/v1/bridges/${segment(id)}/allocations`;
}
function cleanupPath(id: string, allocation: string) {
  input(schema.allocationId, allocation);
  return `${path(id)}/${segment(allocation)}/child-cleanup`;
}
function status(id: string, allocation: string, value: unknown) {
  const result = schema.status(value);
  if (result.bridge_id !== id || result.allocation_id !== allocation)
    throw new ProtocolError('child cleanup changed the requested Bridge or allocation');
  return result;
}
/** Explicit child removal with retained acceptance and current, separately observed progress. */
export class BridgeCleanup extends BridgeOutbox {
  listAllocations(id: string, options: PageOptions = {}) {
    const limit = options.limit ?? 50;
    if (input(positiveInteger, limit) > 100)
      throw new ValidationError('allocation limit exceeds 100');
    const cursor = options.cursor === undefined ? undefined : input(schema.cursor, options.cursor);
    return read(
      this.config,
      query(path(id), { cursor, limit: options.limit }),
      (value) => {
        const result = schema.page(value);
        if (
          result.items.length > limit ||
          result.items.some(
            (item, index) =>
              item.bridge_id !== id || (index > 0 && item.id <= result.items[index - 1]!.id),
          ) ||
          (result.next_cursor !== null && result.next_cursor === cursor)
        )
          throw new ProtocolError(
            'allocation page changed Bridge, ordering, limit or continuation',
          );
        return result;
      },
      options,
    );
  }
  readChildCleanup(id: string, allocation: string, options: OperationOptions = {}) {
    return read(
      this.config,
      cleanupPath(id, allocation),
      (value) => status(id, allocation, value),
      options,
    );
  }
  /** Submit once. Retain this exact request/key after an uncertain reply; 202 confirms durable acceptance. */
  requestChildCleanup(
    id: string,
    allocation: string,
    body: schema.BridgeChildCleanupRequest,
    options: CommandOptions,
  ) {
    const captured = schema.request(input(schema.request, body));
    const capturedOptions = command(options);
    if (options.idempotencyKey.length > 200)
      throw new ValidationError('child cleanup idempotencyKey exceeds 200 characters');
    return read(
      this.config,
      cleanupPath(id, allocation),
      (value) => {
        const result = status(id, allocation, value);
        if (
          result.receipt === null ||
          result.receipt.request.action !== captured.action ||
          result.receipt.request.expected_bridge_revision !== captured.expected_bridge_revision ||
          result.receipt.request.expected_allocation_revision !==
            captured.expected_allocation_revision
        )
          throw new ProtocolError('child cleanup receipt does not match its exact request');
        return result;
      },
      { ...capturedOptions, method: 'POST', body: captured, expectedStatus: 202 },
    );
  }
}
