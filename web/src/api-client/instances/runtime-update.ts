import { boolean, enumeration, nullable, object, optional, text, type Output } from '../schema.js';

/** Runtime image references are opaque deployment values, not semantic versions. */
export const sandboxRuntimeUpdate = object({
  current_version: nullable(text),
  current_version_confirmed: optional(boolean),
  image_origin: optional(
    enumeration('managed_agent', 'managed_desktop', 'user_selected', 'unknown'),
  ),
  latest_version: text,
  update_available: boolean,
  state: enumeration('up_to_date', 'available', 'queued', 'updating', 'failed'),
  can_update: boolean,
  blocked_reason: optional(text),
  target_version: optional(text),
  requested_at: optional(text),
  started_at: optional(text),
  completed_at: optional(text),
  failure: optional(object({ code: text, message: text })),
  schedule: object({ timezone: enumeration('Asia/Singapore'), local_time: enumeration('02:00') }),
  preserves_home: boolean,
});
export type SandboxRuntimeUpdate = Output<typeof sandboxRuntimeUpdate>;

export const updateSandboxRuntimeRequest = object({ target_version: text });
export type UpdateSandboxRuntimeRequest = Output<typeof updateSandboxRuntimeRequest>;
