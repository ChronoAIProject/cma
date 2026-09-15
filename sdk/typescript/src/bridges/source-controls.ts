import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';
import { grantId } from './invoke-schema.js';
import { BridgeWorkspaceCleanup } from './workspace-cleanup.js';
import * as schema from './source-controls-schema.js';

function path(id: string) {
  return `/api/v1/bridge-invoke-grants/${segment(input(grantId, id))}/source-controls`;
}
/** Separate source-sender consent; this does not enable runtime controls. */
export class BridgeSourceControls extends BridgeWorkspaceCleanup {
  /** Null policy means no consent was recorded. Reading never admits consent. */
  readSourceControls(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      path(id),
      (value) => {
        const result = schema.state(value);
        if (result.policy !== null && result.policy.grant_id !== id)
          throw new ProtocolError('source-control policy changed its invoke grant');
        return result;
      },
      options,
    );
  }
  /** Submit once. Retain the exact body/key after an uncertain reply. Historical replay
   * remains a receipt for its original consent and cannot re-enable a revoked policy. */
  setSourceControls(id: string, body: schema.BridgeSourceControlRequest, options: CommandOptions) {
    const captured = schema.request(input(schema.request, body));
    const capturedOptions = command(options);
    if (options.idempotencyKey.length > 200)
      throw new ValidationError('source-control consent idempotencyKey exceeds 200 characters');
    return read(
      this.config,
      path(id),
      (value) => {
        const result = schema.receipt(value);
        if (result.result.grant_id !== id || !schema.sameRequest(result.request, captured))
          throw new ProtocolError('source-control receipt changed the original request or grant');
        return result;
      },
      { ...capturedOptions, method: 'PUT', body: captured, expectedStatus: 200 },
    );
  }
}
