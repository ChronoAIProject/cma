import { capabilities } from './bridges/capabilities-schema.js';
export type {
  BridgeCapabilityCatalog,
  BridgeConnectorCapabilities,
  BridgePlacementCapabilities,
} from './bridges/capabilities-schema.js';
import { BridgeSourceControls } from './bridges/source-controls.js';
export type * from './bridges/workspace-cleanup-schema.js';
export type {
  BridgeSourceControlAction,
  BridgeSourceControlRequest,
  BridgeSourceControlPolicy,
  BridgeSourceControlState,
  BridgeSourceControlReceipt,
  BridgeSourceControlScope,
  BridgeSourceControlApprovalMode,
} from './bridges/source-controls-schema.js';
export type {
  BridgeChildCleanupAction,
  BridgeChildCleanupRequest,
  BridgeChildCleanupReceipt,
  BridgeChildCleanupProgress,
  BridgeChildCleanupHold,
  BridgeChildCleanupStatus,
  BridgeCleanupPlacement,
  BridgeCleanupAllocationState,
  BridgeCleanupAllocation,
  BridgeCleanupAllocationPage,
} from './bridges/cleanup-schema.js';
import { ProtocolError, ValidationError } from './errors.js';
import type { OperationOptions } from './http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
} from './resources.js';
import { positiveInteger } from './schema.js';
import * as schema from './bridges/schema.js';
export type * from './bridges/schema.js';
export type {
  BridgeDelivery,
  BridgeDeliveryStatus,
  BridgeDeliveryAttempt,
  BridgeDeliveryPage,
  BridgeDeliveryRecoveryAction,
  BridgeDeliveryRecoveryRequest,
  BridgeDeliveryRecoveryReceipt,
} from './bridges/outbox-schema.js';
export type {
  BridgeAgentTarget,
  BridgeInvokeGrant,
  BridgeInvokeReceipt,
  BridgeInvokePage,
  BridgeInvokeReadiness,
  BridgeInvokeGrantState,
  BridgeSharedContextConsent,
  BridgeInvokeConsentStatus,
  BridgeInvokePin,
  BridgeInvokeTarget,
  IssueBridgeInvokeGrantRequest,
  RevokeBridgeInvokeGrantRequest,
} from './bridges/invoke-schema.js';
export interface BridgeListOptions extends PageOptions {
  readonly organization?: string;
}
export interface BridgeOwnerOptions extends OperationOptions {
  readonly organization?: string;
}
const root = '/api/v1/bridges';
function path(id: string): string {
  return `${root}/${segment(id)}`;
}
function management(options: CommandOptions) {
  if (typeof options.idempotencyKey === 'string' && options.idempotencyKey.length > 200)
    throw new ValidationError('bridge idempotencyKey must contain at most 200 characters');
  return command(options);
}
function receipt(id: string, expected: number, kind: string) {
  return (value: unknown) => {
    const result = schema.receipt(value);
    if (result.bridge_id !== id || result.revision !== expected + 1 || result.command !== kind)
      throw new ProtocolError('bridge receipt does not match the requested command');
    return result;
  };
}
/** Current bridge management. Channel callback and reply credentials are not dashboard authentication. */
export class Bridges extends BridgeSourceControls {
  list(options: BridgeListOptions = {}) {
    if (options.limit !== undefined && input(positiveInteger, options.limit) > 100)
      throw new ValidationError('bridge limit must be at most 100');
    return read(
      this.config,
      query(root, {
        organization: options.organization,
        cursor: options.cursor,
        limit: options.limit,
      }),
      (value) => {
        const page = schema.page(value);
        if (page.items.length > (options.limit ?? 50))
          throw new ProtocolError('bridge page exceeded its requested limit');
        let previous = options.cursor;
        for (const item of page.items) {
          if (previous !== undefined && item.id <= previous)
            throw new ProtocolError('bridge page is not strictly ordered after its cursor');
          if (
            options.organization !== undefined &&
            (item.owner.kind !== 'organization' || item.owner.subject !== options.organization)
          )
            throw new ProtocolError('bridge page owner does not match the requested organization');
          previous = item.id;
        }
        if (
          page.next_cursor !== undefined &&
          (page.items.length === 0 || page.next_cursor !== previous)
        )
          throw new ProtocolError('bridge continuation does not match its page');
        return page;
      },
      options,
    );
  }
  read(id: string, options: OperationOptions = {}) {
    return read(this.config, path(id), identity(schema.bridge, id), options);
  }
  choices(options: BridgeOwnerOptions = {}) {
    return read(
      this.config,
      query('/api/v1/bridge-connectors', { organization: options.organization }),
      schema.choices,
      options,
    );
  }
  /** Source/fixture capabilities and current deployment switches, without provider requests. */
  capabilities(options: BridgeOwnerOptions = {}) {
    return read(
      this.config,
      query('/api/v1/bridge-connectors/capabilities', { organization: options.organization }),
      capabilities,
      options,
    );
  }
  configuration(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `${path(id)}/configuration`,
      (value) => {
        const result = schema.detail(value);
        if (
          result.bridge.id !== id ||
          !result.bridge.can_manage ||
          result.config.binding.bot_id !== result.bridge.bot_id ||
          result.config.binding.platform !== result.bridge.platform
        )
          throw new ProtocolError('bridge configuration does not match the authorized bridge');
        return result;
      },
      options,
    );
  }
  create(body: schema.CreateBridgeRequest, options: CommandOptions) {
    return read(
      this.config,
      root,
      (value) => {
        const result = schema.receipt(value);
        if (
          result.revision !== 1 ||
          result.config_revision !== 1 ||
          result.command !== 'create' ||
          result.owner.kind !== body.owner.kind ||
          result.owner.subject !== body.owner.subject
        )
          throw new ProtocolError(
            'bridge creation receipt does not match its owner or initial revision',
          );
        return result;
      },
      {
        ...management(options),
        method: 'POST',
        body: input(schema.createRequest, body),
        expectedStatus: 201,
      },
    );
  }
  update(id: string, body: schema.UpdateBridgeRequest, options: CommandOptions) {
    return read(
      this.config,
      `${path(id)}/configuration`,
      receipt(id, body.expected_revision, 'update'),
      { ...management(options), method: 'PUT', body: input(schema.updateRequest, body) },
    );
  }
  control(id: string, body: schema.ControlBridgeRequest, options: CommandOptions) {
    return read(
      this.config,
      `${path(id)}/commands`,
      receipt(id, body.expected_revision, body.command.kind),
      { ...management(options), method: 'POST', body: input(schema.controlRequest, body) },
    );
  }
  validate(id: string, expectedRevision: number, options: OperationOptions = {}) {
    return read(
      this.config,
      `${path(id)}/validate`,
      (value) => {
        const result = schema.validation(value);
        if (
          result.bridge_id !== id ||
          result.revision !== expectedRevision ||
          result.config_revision > result.revision ||
          !result.valid
        )
          throw new ProtocolError('bridge validation does not match its requested revision');
        return result;
      },
      {
        ...options,
        method: 'POST',
        body: { expected_revision: input(positiveInteger, expectedRevision) },
      },
    );
  }
  /** Import the exact selected key into CMA control-plane custody; this method never retains it. */
  importKey(
    id: string,
    expectedRevision: number,
    key: string,
    options: OperationOptions & { readonly retainControlPlaneAuthority?: boolean } = {},
  ) {
    if (typeof key !== 'string' || key.length === 0 || key.length > 4096)
      throw new ValidationError('selected bridge key must contain 1 through 4096 characters');
    if (
      options.retainControlPlaneAuthority !== undefined &&
      typeof options.retainControlPlaneAuthority !== 'boolean'
    )
      throw new ValidationError('bridge background authority consent must be a boolean');
    return read(this.config, `${path(id)}/credential`, schema.credentialStatus, {
      ...options,
      method: 'PUT',
      body: {
        expected_revision: input(positiveInteger, expectedRevision),
        key,
        ...(options.retainControlPlaneAuthority === undefined
          ? {}
          : {
              retain_control_plane_authority: options.retainControlPlaneAuthority,
            }),
      },
    });
  }
  revokeKey(
    id: string,
    expectedRevision: number,
    expectedCustodyRevision: number,
    options: CommandOptions,
  ) {
    return read(this.config, `${path(id)}/credential/revoke`, schema.credentialStatus, {
      ...management(options),
      method: 'POST',
      body: {
        expected_revision: input(positiveInteger, expectedRevision),
        expected_custody_revision: input(positiveInteger, expectedCustodyRevision),
      },
    });
  }
}
