import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
} from '../resources.js';
import { positiveInteger } from '../schema.js';
import * as schema from './invoke-schema.js';

function bridgePath(id: string): string {
  return `/api/v1/bridges/${segment(input(schema.boundedId, id))}`;
}
function grantPath(id: string): string {
  return `/api/v1/bridge-invoke-grants/${segment(input(schema.grantId, id))}`;
}
function management(options: CommandOptions) {
  if (typeof options.idempotencyKey === 'string' && options.idempotencyKey.length > 200)
    throw new ValidationError('bridge invoke idempotencyKey exceeds 200 characters');
  return command(options);
}

/** Explicit consent management under current dashboard authority. */
export class BridgeInvokes {
  constructor(protected readonly config: ResolvedConfig) {}
  /** Ordered current authorized history, including revoked grants. Default limit 50; maximum 100. */
  listInvokeGrants(id: string, options: PageOptions = {}) {
    if (options.limit !== undefined && input(positiveInteger, options.limit) > 100)
      throw new ValidationError('bridge invoke limit exceeds 100');
    if (options.cursor !== undefined) input(schema.grantId, options.cursor);
    return read(
      this.config,
      query(`${bridgePath(id)}/invoke-grants`, { cursor: options.cursor, limit: options.limit }),
      (value) => {
        const page = schema.page(value);
        if (page.items.length > (options.limit ?? 50))
          throw new ProtocolError('bridge invoke page exceeded the requested limit');
        let previous = options.cursor;
        let owner: schema.BridgeInvokeGrant['owner'] | undefined;
        for (const grant of page.items) {
          if (
            grant.pin.bridge_id !== id ||
            (previous !== undefined && grant.id <= previous) ||
            (owner !== undefined &&
              (owner.kind !== grant.owner.kind || owner.subject !== grant.owner.subject))
          )
            throw new ProtocolError('bridge invoke page changed bridge/owner or failed to advance');
          previous = grant.id;
          owner = grant.owner;
        }
        if (page.next_cursor !== null && (page.items.length === 0 || page.next_cursor !== previous))
          throw new ProtocolError('bridge invoke continuation does not match its page');
        return page;
      },
      options,
    );
  }
  readInvokeGrant(id: string, options: OperationOptions = {}) {
    return read(this.config, grantPath(id), identity(schema.grant, id), options);
  }
  /** Consent observation only; current custody, enablement and execution authority are checked separately. */
  invokeReadiness(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `${bridgePath(id)}/invoke-readiness`,
      (value) => {
        const result = schema.readiness(value);
        if (result.bridge_id !== id)
          throw new ProtocolError('bridge invoke readiness changed its requested bridge');
        return result;
      },
      options,
    );
  }
  /** Explicit retained-context consent. After cancellation or response loss, retry the unchanged body and key. */
  issueInvokeGrant(
    id: string,
    body: schema.IssueBridgeInvokeGrantRequest,
    options: CommandOptions,
  ) {
    const request = schema.issueRequest(input(schema.issueRequest, body));
    return read(
      this.config,
      `${bridgePath(id)}/invoke-grants`,
      (value) => {
        const result = schema.receipt(value);
        if (
          result.bridge_id !== id ||
          !schema.sameTarget(result.result.target.ids, request.target) ||
          result.result.consent !== request.shared_context ||
          result.result.state.kind !== 'active' ||
          result.result.pin.config_revision > request.expected_bridge_revision
        )
          throw new ProtocolError(
            'issued bridge invoke grant does not match the requested bridge, target or consent',
          );
        return result;
      },
      { ...management(options), method: 'POST', body: request, expectedStatus: 201 },
    );
  }
  /** Terminal revocation. Aborting the signal stops observation and cannot roll back a server commit. */
  revokeInvokeGrant(
    id: string,
    body: schema.RevokeBridgeInvokeGrantRequest,
    options: CommandOptions,
  ) {
    const request = schema.revokeRequest(input(schema.revokeRequest, body));
    return read(
      this.config,
      `${grantPath(id)}/revoke`,
      (value) => {
        const result = schema.receipt(value);
        if (
          result.result.id !== id ||
          result.result.state.kind !== 'revoked' ||
          result.result.revision !== request.expected_revision + 1 ||
          result.result.pin.config_revision > request.expected_bridge_revision
        )
          throw new ProtocolError(
            'revoked bridge invoke grant does not match the requested identity or revision',
          );
        return result;
      },
      { ...management(options), method: 'POST', body: request },
    );
  }
}
