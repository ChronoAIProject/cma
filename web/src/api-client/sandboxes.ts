import {
  creationOutcome,
  creationOutcomeRequest,
  type CreationOutcomeRequest,
} from './instances/creation-outcome.js';
import { sandboxCapacity } from './instances/capacity.js';
import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { ProtocolError, ValidationError } from './errors.js';
import { array, boolean, integer, object, type Output } from './schema.js';
import {
  command,
  identity,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type EventOptions,
  type PageOptions,
} from './resources.js';
import {
  createSandboxRequest,
  grantSandboxServiceAccessRequest,
  sandbox,
  sandboxDetail,
  sandboxStatus,
  sandboxSummary,
  type CreateSandboxRequest,
  type GrantSandboxServiceAccessRequest,
  type SandboxStatus,
} from './instances/types.js';
import { sandboxEvent } from './instances/events.js';
import {
  retainedRecoveryAssessment,
  recoverRetainedRequest,
  retainedReopenReceipt,
  type RecoverRetainedRequest,
} from './instances/retained-recovery.js';
import {
  updateSandboxRuntimeRequest,
  type UpdateSandboxRuntimeRequest,
} from './instances/runtime-update.js';
import { accessibleSandboxDetail, accessibleSandboxSummary } from './instances/shared.js';
import {
  sandboxCommandReceipt,
  sandboxCommandRequest,
  type SandboxCommandRequest,
} from './instances/commands.js';

const alwaysOnRequest = object({ always_on: boolean, expected_revision: integer });
/** Owners may send false only; true receives 403 always_on_approval_required. */
export type AlwaysOnRequest = Output<typeof alwaysOnRequest>;

export interface SandboxListOptions extends PageOptions {
  readonly status?: SandboxStatus;
  /** Exclude retired history before pagination; defaults to false. */
  readonly excludeRetired?: boolean;
}

export interface AccessibleSandboxListOptions extends SandboxListOptions {
  readonly organization?: string;
  readonly shared?: boolean;
}

/** Owner-scoped Sandbox provisioning, hierarchy reads, and lifecycle commands. */
export class Sandboxes {
  constructor(private readonly config: ResolvedConfig) {}

  lookupCreation(body: CreationOutcomeRequest, options: OperationOptions = {}) {
    input(creationOutcomeRequest, body);
    return read(this.config, '/api/v1/sandbox-creation-outcomes/lookup', creationOutcome, {
      ...options,
      method: 'POST',
      body,
    });
  }

  capacity(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/sandboxes/capacity', sandboxCapacity, options);
  }

  /** Admit a durable personal/shared lifecycle intent with a current revision. */
  submitCommand(id: string, body: SandboxCommandRequest, options: CommandOptions) {
    // Capture the JSON before request ID resolution yields. Forward extra fields
    // so the server can continue to return its typed input refusal.
    let snapshot: unknown;
    let submitted: SandboxCommandRequest;
    try {
      submitted = sandboxCommandRequest(body);
      snapshot = JSON.parse(JSON.stringify(body)) as unknown;
      const encoded = sandboxCommandRequest(snapshot);
      if (
        encoded.command !== submitted.command ||
        encoded.expected_revision !== submitted.expected_revision
      )
        throw new ProtocolError('Sandbox command changed during JSON encoding');
    } catch (cause) {
      throw new ValidationError(cause instanceof Error ? cause.message : 'invalid request body', {
        cause,
      });
    }
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/commands`,
      (value) => {
        const result = sandboxCommandReceipt(value);
        if (
          result.sandbox_id !== id ||
          result.command !== submitted.command ||
          result.expected_revision !== submitted.expected_revision
        )
          throw new ProtocolError('Sandbox command receipt did not match the request');
        return result;
      },
      { ...command(options), method: 'POST', body: snapshot, expectedStatus: 202 },
    );
  }

  /** Recover a receipt while the requesting actor retains target visibility. */
  readCommand(commandId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandbox-commands/${segment(commandId)}`,
      (value) => {
        const result = sandboxCommandReceipt(value);
        if (result.command_id !== commandId)
          throw new ProtocolError('Sandbox command identity did not match the request');
        return result;
      },
      options,
    );
  }

  /** Personal inventory by default, or current organization / incoming shared scope. */
  listAccessible(options: AccessibleSandboxListOptions = {}) {
    if (options.shared !== undefined) input(boolean, options.shared);
    if (options.shared === true && options.organization !== undefined)
      throw new ValidationError('Choose an organization or Shared with me, not both.');
    if (options.status !== undefined) input(sandboxStatus, options.status);
    if (options.excludeRetired !== undefined) input(boolean, options.excludeRetired);
    return read(
      this.config,
      query('/api/v1/sandboxes', {
        cursor: options.cursor,
        limit: options.limit,
        status: options.status,
        exclude_retired: options.excludeRetired === true ? 'true' : undefined,
        organization: options.organization,
        shared: options.shared === true ? 'true' : undefined,
      }),
      page(accessibleSandboxSummary),
      options,
    );
  }

  list(options: SandboxListOptions = {}) {
    if (options.status !== undefined) input(sandboxStatus, options.status);
    if (options.excludeRetired !== undefined) input(boolean, options.excludeRetired);
    return read(
      this.config,
      query('/api/v1/sandboxes', {
        cursor: options.cursor,
        limit: options.limit,
        status: options.status,
        exclude_retired: options.excludeRetired === true ? 'true' : undefined,
      }),
      page(sandboxSummary),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}`,
      identity(sandboxDetail, id),
      options,
    );
  }

  /** Personal detail or safe shared metadata and authorized child counts. */
  readAccessible(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}`,
      identity(accessibleSandboxDetail, id),
      options,
    );
  }

  create(body: CreateSandboxRequest, options: CommandOptions) {
    return read(this.config, '/api/v1/sandboxes', sandbox, {
      ...command(options),
      method: 'POST',
      body: input(createSandboxRequest, body),
      expectedStatus: 201,
    });
  }

  delete(id: string, options: CommandOptions) {
    return this.lifecycle(id, '', 'DELETE', options);
  }
  pause(id: string, options: CommandOptions) {
    return this.lifecycle(id, '/pause', 'POST', options);
  }
  revive(id: string, options: CommandOptions) {
    return this.lifecycle(id, '/revive', 'POST', options);
  }

  /**
   * Relinquish an existing exception with `always_on: false`. Unset/false is a
   * revision-checked no-op. `true` returns 403 `always_on_approval_required`.
   * Only admin.setSandboxAlwaysOn can approve an exception.
   */
  setAlwaysOn(id: string, body: AlwaysOnRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/sandboxes/${segment(id)}/always-on`, identity(sandbox, id), {
      ...command(options),
      method: 'POST',
      body: input(alwaysOnRequest, body),
      expectedStatus: 200,
    });
  }

  /** Read fresh retained-runtime facts without admitting or interrupting work. */
  inspectRuntimeRecovery(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/runtime-recovery`,
      retainedRecoveryAssessment,
      options,
    );
  }

  /** Explicit owner consent; server re-proves exact retained authority and idle history. */
  recoverRetained(id: string, body: RecoverRetainedRequest, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/runtime-recovery`,
      retainedReopenReceipt,
      {
        ...command(options),
        method: 'POST',
        body: input(recoverRetainedRequest, body),
        expectedStatus: 202,
      },
    );
  }

  /** Queue an exact deployment runtime image; active work finishes before rotation. */
  updateRuntime(id: string, body: UpdateSandboxRuntimeRequest, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/runtime-update`,
      (value) => {
        const result = identity(sandbox, id)(value);
        if (result.runtime_update === undefined)
          throw new ProtocolError('Runtime update response omitted its status');
        return { ...result, runtime_update: result.runtime_update };
      },
      {
        ...command(options),
        method: 'POST',
        body: input(updateSandboxRuntimeRequest, body),
        expectedStatus: [200, 202],
      },
    );
  }

  grantServiceAccess(id: string, body: GrantSandboxServiceAccessRequest, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/service-access`,
      identity(sandbox, id),
      {
        ...command(options),
        method: 'POST',
        body: input(grantSandboxServiceAccessRequest, body),
        expectedStatus: 200,
      },
    );
  }

  events(id: string, options: EventOptions = {}) {
    return read(
      this.config,
      query(`/api/v1/sandboxes/${segment(id)}/events`, {
        after: options.after,
        limit: options.limit,
      }),
      array((value) => {
        const event = sandboxEvent(value);
        if (event.sandbox_id !== id)
          throw new ProtocolError('event Sandbox identity did not match the request');
        return event;
      }),
      options,
    );
  }

  private lifecycle(id: string, suffix: string, method: string, options: CommandOptions) {
    return read(this.config, `/api/v1/sandboxes/${segment(id)}${suffix}`, identity(sandbox, id), {
      ...command(options),
      method,
      expectedStatus: 202,
    });
  }
}
