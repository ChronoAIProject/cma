import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';
import { boundedId } from './invoke-schema.js';
import { BridgeCleanup } from './cleanup.js';
import * as schema from './workspace-cleanup-schema.js';
import { allocationId } from './cleanup-schema.js';

function path(id: string) {
  input(boundedId, id);
  return `/api/v1/bridges/${segment(id)}/allocations`;
}
function cleanupPath(id: string, allocation: string) {
  input(allocationId, allocation);
  return `${path(id)}/${segment(allocation)}/workspace-cleanup`;
}
function status(id: string, allocation: string, value: unknown) {
  const result = schema.status(value);
  if (result.bridge_id !== id || result.allocation_id !== allocation)
    throw new ProtocolError('Workspace cleanup changed the requested Bridge or allocation');
  return result;
}
/** Separately authorized removal of an allocation-owned Workspace after child cleanup completes. */
export class BridgeWorkspaceCleanup extends BridgeCleanup {
  readWorkspaceCleanup(id: string, allocation: string, options: OperationOptions = {}) {
    return read(
      this.config,
      cleanupPath(id, allocation),
      (value) => status(id, allocation, value),
      options,
    );
  }
  /** Submit once. Retain this exact request/key after an uncertain reply; 202 confirms durable acceptance. */
  requestWorkspaceCleanup(
    id: string,
    allocation: string,
    body: schema.BridgeWorkspaceCleanupRequest,
    options: CommandOptions,
  ) {
    const captured = schema.request(input(schema.request, body));
    const capturedOptions = command(options);
    if (options.idempotencyKey.length > 200)
      throw new ValidationError('Workspace cleanup idempotencyKey exceeds 200 characters');
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
            captured.expected_allocation_revision ||
          result.receipt.request.expected_workspace_revision !==
            captured.expected_workspace_revision ||
          result.receipt.request.child_cleanup_receipt_id !== captured.child_cleanup_receipt_id
        )
          throw new ProtocolError('Workspace cleanup receipt does not match its exact request');
        return result;
      },
      { ...capturedOptions, method: 'POST', body: captured, expectedStatus: 202 },
    );
  }
}
