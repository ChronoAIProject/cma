import {
  warmPoolList,
  warmPoolView,
  validateWarmPoolTarget,
  type ConfigureWarmPoolBody,
  type ResizeWarmPoolBody,
} from './warm-pools.js';
export type * from './warm-pools.js';
import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
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
  activityRecord,
  adminPage,
  adminProfileSummary,
  adminSandbox,
  adminSandboxDetail,
  adminSandboxSummary,
  type ActivityAction,
} from './types.js';

export type * from './types.js';
export type * from './settings.js';

export interface AdminSandboxListOptions extends PageOptions {
  readonly owner?: string;
  readonly status?: SandboxStatus;
  readonly runtime_version?: string;
}
export interface AdminProfileListOptions extends PageOptions {
  readonly kind?: ProfileKind;
  readonly visibility?: ProfileVisibility;
  readonly owner?: string;
}
export interface ActivityListOptions extends PageOptions {
  readonly actor?: string;
  readonly action?: ActivityAction;
}

const autoPausePreview = object({ candidates: integer });
export type AutoPausePreview = Output<typeof autoPausePreview>;
const pauseIdleNowRequest = object({ confirmed: boolean });
const pauseIdleNow = object({ scheduled: integer, already_satisfied: integer, skipped: integer });
export type PauseIdleNow = Output<typeof pauseIdleNow>;
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
  previewAutoPause(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/auto-pause/preview', autoPausePreview, options);
  }
  pauseIdleNow(confirmed: boolean, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/auto-pause/pause-idle-now', pauseIdleNow, {
      ...options,
      method: 'POST',
      body: input(pauseIdleNowRequest, { confirmed }),
    });
  }
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
  listActivity(options: ActivityListOptions = {}) {
    bound(options);
    if (options.action !== undefined) input(activityAction, options.action);
    return read(
      this.config,
      query('/api/v1/admin/activity', {
        actor: options.actor,
        action: options.action,
        cursor: options.cursor,
        limit: options.limit,
      }),
      adminPage(activityRecord),
      options,
    );
  }
}
