import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { sandboxEvent } from '../instances/events.js';
import type { SandboxStatus } from '../instances/types.js';
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
import { array } from '../schema.js';
import {
  replaceSettingsRequest,
  storedPlatformSettings,
  type ReplaceSettingsRequest,
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
