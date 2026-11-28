import { autoPausePreview, pauseIdleNow, pauseIdleNowRequest } from './auto-pause.js';
export type { AutoPausePreview, PauseIdleNow } from './auto-pause.js';
import {
  pendingHomesReconciliation,
  reconcilePendingHomesRequest,
  type ReconcilePendingHomesRequest,
} from './pending-homes.js';
export type * from './pending-homes.js';
import {
  warmPoolList,
  warmPoolView,
  validateWarmPoolTarget,
  type ConfigureWarmPoolBody,
  type ResizeWarmPoolBody,
} from './warm-pools.js';
export type * from './warm-pools.js';
import type { ResolvedConfig } from '../config.js';
import { bindingHoldOverview } from './binding-holds.js';
import * as runtimeMigration from './runtime-migration.js';
export type * from './runtime-migration.js';
export type * from './binding-holds.js';
import { platformSpend, spendObservationRequest, type SpendObservationRequest } from './spend.js';
import {
  admissionCeilingsState,
  replaceAdmissionCeilingsRequest,
  type ReplaceAdmissionCeilingsRequest,
  admissionControl,
  admissionControlRead,
  changeAdmissionRequest,
  type ChangeAdmissionRequest,
} from './admission.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { sandboxEvent } from '../instances/events.js';
import { sandbox, type SandboxStatus } from '../instances/types.js';
import {
  updateSandboxRuntimeRequest,
  type UpdateSandboxRuntimeRequest,
} from '../instances/runtime-update.js';
import { profileKind, type ProfileKind, type ProfileVisibility } from '../profiles/types.js';
import {
  identity,
  input,
  query,
  read,
  segment,
  type EventOptions,
  type PageOptions,
} from '../resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  text,
  type Output,
} from '../schema.js';
import {
  patchSettingsRequest,
  replaceSettingsRequest,
  resetSettingsRequest,
  storedPlatformSettings,
  type PatchSettingsRequest,
  type ReplaceSettingsRequest,
  type ResetSettingsRequest,
} from './settings.js';
import {
  activityAction,
  activityChannel,
  activityRecord,
  activitySummary,
  adminPage,
  adminProfileSummary,
  adminSandbox,
  adminSandboxDetail,
  adminSandboxSummary,
  type ActivityAction,
  type ActivityChannel,
  type ActivityResult,
} from './types.js';

export type * from './types.js';
export type * from './settings.js';
export type * from './admission.js';
export type * from './spend.js';

export interface AdminSandboxListOptions extends PageOptions {
  readonly owner?: string;
  readonly status?: SandboxStatus;
  readonly runtime_version?: string;
  /** True selects explicit exceptions; false includes unset preferences. */
  readonly always_on?: boolean;
}
export interface AdminProfileListOptions extends PageOptions {
  readonly kind?: ProfileKind;
  readonly visibility?: ProfileVisibility;
  readonly owner?: string;
}
export interface ActivityListOptions extends PageOptions {
  readonly actor?: string;
  readonly action?: ActivityAction;
  readonly channel?: ActivityChannel;
  readonly result?: ActivityResult;
  readonly recordedFrom?: string;
  readonly recordedBefore?: string;
  readonly excludeSuccessfulReads?: boolean;
}

const setSandboxAlwaysOnRequest = object({ always_on: boolean, expected_revision: integer });
export type SetSandboxAlwaysOnRequest = Output<typeof setSandboxAlwaysOnRequest>;

const runtimeInventory = object({
  observed_at: text,
  runtimes: array(
    object({
      runtime_id: text,
      generation: integer,
      owner_reference: nullable(text),
      status: enumeration('starting', 'ready', 'unavailable'),
    }),
  ),
});
export type RuntimeInventory = Output<typeof runtimeInventory>;

function bound(options: { readonly limit?: number; readonly cursor?: string }) {
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 200)
  )
    throw new ValidationError('limit must be between 1 and 200');
  if (options.cursor === '') throw new ValidationError('cursor must not be empty');
}

/** Public operator endpoints. Settings writes use revision CAS; no command is automatically retried. */
export class Administration {
  constructor(readonly config: ResolvedConfig) {}

  bindingHolds(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-binding-holds', bindingHoldOverview, {
      ...options,
      cache: 'no-store',
    });
  }

  listWarmPools(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/warm-pools', warmPoolList, options);
  }
  configureWarmPool(
    body: ConfigureWarmPoolBody,
    options: OperationOptions & { readonly idempotencyKey: string },
  ) {
    validateWarmPoolTarget(body);
    if (
      !options.idempotencyKey ||
      options.idempotencyKey.length > 128 ||
      !/^[\x21-\x7e]+$/.test(options.idempotencyKey)
    )
      throw new ValidationError('a valid Idempotency-Key is required');
    return read(this.config, '/api/v1/admin/warm-pools', warmPoolView, {
      ...options,
      method: 'POST',
      expectedStatus: 202,
      body,
      headers: { 'Idempotency-Key': options.idempotencyKey },
    });
  }
  resizeWarmPool(id: string, body: ResizeWarmPoolBody, options: OperationOptions = {}) {
    validateWarmPoolTarget(body);
    return read(
      this.config,
      `/api/v1/admin/warm-pools/${segment(id)}`,
      identity(warmPoolView, id),
      {
        ...options,
        method: 'PUT',
        expectedStatus: 202,
        body,
      },
    );
  }
  listSandboxes(options: AdminSandboxListOptions = {}) {
    bound(options);
    return read(
      this.config,
      query('/api/v1/admin/sandboxes', {
        owner: options.owner,
        status: options.status,
        runtime_version: options.runtime_version,
        always_on: options.always_on?.toString(),
        cursor: options.cursor,
        limit: options.limit,
      }),
      adminPage(adminSandboxSummary),
      options,
    );
  }
  readSandbox(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/admin/sandboxes/${segment(id)}`,
      (value) => {
        const detail = adminSandboxDetail(value);
        if (detail.sandbox.id !== id)
          throw new ProtocolError('Sandbox identity did not match the request');
        return detail;
      },
      options,
    );
  }
  async sandboxEvents(id: string, options: EventOptions = {}) {
    bound(options);
    const events = await read(
      this.config,
      query(`/api/v1/admin/sandboxes/${segment(id)}/events`, {
        after: options.after,
        limit: options.limit,
      }),
      array(sandboxEvent),
      options,
    );
    if (events.some((event) => event.sandbox_id !== id))
      throw new ProtocolError('Sandbox event identity did not match the request');
    return events;
  }
  retireSandbox(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/admin/sandboxes/${segment(id)}/retire`,
      identity(adminSandbox, id),
      { ...options, method: 'POST' },
    );
  }
  /** Queue the managed runtime update across owners without interrupting active work. */
  updateSandboxRuntime(
    id: string,
    body: UpdateSandboxRuntimeRequest,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `/api/v1/admin/sandboxes/${segment(id)}/runtime-update`,
      (value) => {
        const result = identity(sandbox, id)(value);
        if (result.runtime_update === undefined)
          throw new ProtocolError('Runtime update response omitted its status');
        return { ...result, runtime_update: result.runtime_update };
      },
      {
        ...options,
        method: 'POST',
        expectedStatus: 202,
        body: input(updateSandboxRuntimeRequest, body),
      },
    );
  }
  runtimeActivity(id: string, options: OperationOptions = {}) {
    return runtimeMigration.readRuntimeActivity(this.config, id, options);
  }

  migrateSandboxRuntime(
    id: string,
    body: runtimeMigration.CustomRuntimeMigrationRequest,
    options: OperationOptions = {},
  ) {
    return runtimeMigration.requestRuntimeMigration(this.config, id, body, options);
  }

  listProfiles(options: AdminProfileListOptions = {}) {
    bound(options);
    return read(
      this.config,
      query('/api/v1/admin/profiles', {
        kind: options.kind,
        visibility: options.visibility,
        owner: options.owner,
        cursor: options.cursor,
        limit: options.limit,
      }),
      adminPage(adminProfileSummary),
      options,
    );
  }
  retireProfile(kind: ProfileKind, id: string, options: OperationOptions = {}) {
    input(profileKind, kind);
    return read(
      this.config,
      `/api/v1/admin/profiles/${kind}/${segment(id)}/retire`,
      (value) => {
        const profile = identity(adminProfileSummary, id)(value);
        if (profile.kind !== kind)
          throw new ProtocolError('Profile kind did not match the request');
        return profile;
      },
      { ...options, method: 'POST' },
    );
  }
  readSettings(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/settings', storedPlatformSettings, options);
  }
  readSpend(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/spend', platformSpend, options);
  }
  /** Record an explicit cloud observation; uncertain commands are never automatically retried. */
  recordSpendObservation(body: SpendObservationRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/spend/observations', platformSpend, {
      ...options,
      method: 'POST',
      body: input(spendObservationRequest, body),
    });
  }
  reconcilePendingHomes(body: ReconcilePendingHomesRequest, options: OperationOptions = {}) {
    return read(
      this.config,
      '/api/v1/admin/admission/pending-homes/reconcile',
      pendingHomesReconciliation,
      {
        ...options,
        method: 'POST',
        body: input(reconcilePendingHomesRequest, body),
      },
    );
  }
  readAdmission(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/admission', admissionControlRead, options);
  }
  /** Stop fresh work. A repeated stop returns the first operator's state. */
  stopAdmission(body: ChangeAdmissionRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/admission/stop', admissionControl, {
      ...options,
      method: 'POST',
      body: input(changeAdmissionRequest, body),
    });
  }
  /** Resume only the observed stop revision; read again after an uncertain result. */
  resumeAdmission(body: ChangeAdmissionRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/admission/resume', admissionControl, {
      ...options,
      method: 'POST',
      body: input(changeAdmissionRequest, body),
    });
  }
  /** Replace independently revised capacity policy; null leaves a ceiling unlimited. */
  replaceAdmissionCeilings(body: ReplaceAdmissionCeilingsRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/admission/ceilings', admissionCeilingsState, {
      ...options,
      method: 'PUT',
      body: input(replaceAdmissionCeilingsRequest, body),
    });
  }
  replaceSettings(body: ReplaceSettingsRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/settings', storedPlatformSettings, {
      ...options,
      method: 'PUT',
      body: input(replaceSettingsRequest, body),
    });
  }
  patchSettings(body: PatchSettingsRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/settings', storedPlatformSettings, {
      ...options,
      method: 'PATCH',
      body: input(patchSettingsRequest, body),
    });
  }
  resetSettings(body: ResetSettingsRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/settings/reset', storedPlatformSettings, {
      ...options,
      method: 'POST',
      body: input(resetSettingsRequest, body),
    });
  }
  /** One bounded page under saved policy. Empty cursor retries the first uncertain candidate. */
  previewAutoPause(options: PageOptions = {}) {
    autoPauseBound(options);
    return read(
      this.config,
      query('/api/v1/admin/auto-pause/preview', {
        cursor: options.cursor,
        limit: options.limit,
      }),
      autoPausePreview,
      options,
    );
  }
  /** Explicitly schedule one batch; never automatically repeat an uncertain batch. */
  pauseIdleNow(confirmed: boolean, options: PageOptions = {}) {
    autoPauseBound(options);
    return read(this.config, '/api/v1/admin/auto-pause/pause-idle-now', pauseIdleNow, {
      ...options,
      method: 'POST',
      body: input(pauseIdleNowRequest, { confirmed, cursor: options.cursor, limit: options.limit }),
    });
  }
  /** Approve or revoke an exception with revision CAS and an administrator audit record. */
  setSandboxAlwaysOn(id: string, body: SetSandboxAlwaysOnRequest, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/admin/sandboxes/${segment(id)}/always-on`,
      identity(sandbox, id),
      {
        ...options,
        method: 'POST',
        body: input(setSandboxAlwaysOnRequest, body),
        expectedStatus: 200,
      },
    );
  }
  pauseTriggers(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/triggers/pause', storedPlatformSettings, {
      ...options,
      method: 'POST',
    });
  }
  resumeTriggers(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/triggers/resume', storedPlatformSettings, {
      ...options,
      method: 'POST',
    });
  }
  readRuntimeInventory(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-inventory', runtimeInventory, options);
  }
  searchActivityActors(textQuery: string, options: OperationOptions = {}) {
    return read(
      this.config,
      query('/api/v1/admin/activity/actors', { q: textQuery }),
      array(object({ subject: text, display_name: nullable(text) })),
      options,
    );
  }
  /** Table fields only; use listActivity for the complete retained audit details. */
  listActivitySummary(options: ActivityListOptions = {}) {
    bound(options);
    if (options.action !== undefined) input(activityAction, options.action);
    if (options.channel !== undefined) input(activityChannel, options.channel);
    const request = (excludeSuccessfulReads: boolean) =>
      read(
        this.config,
        query('/api/v1/admin/activity', {
          view: 'summary',
          actor: options.actor,
          action: options.action,
          channel: options.channel,
          result: options.result,
          recorded_from: options.recordedFrom,
          recorded_before: options.recordedBefore,
          exclude_successful_reads: excludeSuccessfulReads ? 'true' : undefined,
          cursor: options.cursor,
          limit: options.limit,
        }),
        adminPage(activitySummary),
        options,
      );
    if (options.excludeSuccessfulReads !== true) return request(false);
    return request(true).catch((error: unknown) => {
      // Older Agent Managers reject unknown query options during a mixed rollout.
      if (
        error instanceof ApiError &&
        error.status === 400 &&
        error.code === 'invalid_admin_request' &&
        !options.signal?.aborted
      ) {
        return request(false);
      }
      throw error;
    });
  }
  listActivity(options: ActivityListOptions = {}) {
    bound(options);
    if (options.action !== undefined) input(activityAction, options.action);
    if (options.channel !== undefined) input(activityChannel, options.channel);
    return read(
      this.config,
      query('/api/v1/admin/activity', {
        actor: options.actor,
        action: options.action,
        channel: options.channel,
        result: options.result,
        recorded_from: options.recordedFrom,
        recorded_before: options.recordedBefore,
        exclude_successful_reads: options.excludeSuccessfulReads === true ? 'true' : undefined,
        cursor: options.cursor,
        limit: options.limit,
      }),
      adminPage(activityRecord),
      options,
    );
  }
}

function autoPauseBound(options: PageOptions) {
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 100)
  )
    throw new ValidationError('auto-pause batch limit must be between 1 and 100');
}
