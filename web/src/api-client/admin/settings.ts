import { ProtocolError } from '../errors.js';
import { profileReference } from '../profiles/types.js';
import {
  boolean,
  dictionary,
  integer,
  nullable,
  object,
  optional,
  record,
  text,
  type Output,
} from '../schema.js';

const u32 = (value: unknown) => {
  const number = integer(value);
  if (number > 4_294_967_295) throw new ProtocolError('expected an unsigned 32-bit integer');
  return number;
};
const pauseSeconds = (value: unknown) => {
  const number = integer(value);
  if (number < 60 || number > 86_400)
    throw new ProtocolError('pause_after_seconds must be between 60 and 86400');
  return number;
};
const sections = {
  usage_reporting: object({ measured_visible: boolean }),
  defaults: object({
    sandbox_profile: profileReference,
    workspace_profile: profileReference,
    agent_profile: profileReference,
  }),
  inactivity: object({
    auto_pause_enabled: optional(boolean),
    pause_after_seconds: pauseSeconds,
    grace_started_at: optional(nullable(text)),
  }),
  retention: object({ agent_log_days: u32 }),
  limits: object({
    max_sandboxes_per_user: u32,
    visitor_max_sandboxes: u32,
    visitor_max_launches_per_day: u32,
  }),
  triggers: object({ enabled: boolean, orchestration_paused: boolean }),
  marketplace: object({ enabled: boolean }),
};
const knownSettings = object({ ...sections, usage_reporting: optional(sections.usage_reporting) });
type KnownSettings = Output<typeof knownSettings>;
export type PlatformSettings = KnownSettings & Readonly<Record<string, unknown>>;
export function platformSettings(value: unknown): PlatformSettings {
  const known = knownSettings(value);
  const raw = record(value);
  return {
    ...raw,
    ...known,
    ...(known.usage_reporting === undefined
      ? {}
      : { usage_reporting: { ...record(raw.usage_reporting), ...known.usage_reporting } }),
    defaults: {
      ...record(raw.defaults),
      ...known.defaults,
      sandbox_profile: {
        ...record(record(raw.defaults).sandbox_profile),
        ...known.defaults.sandbox_profile,
      },
      workspace_profile: {
        ...record(record(raw.defaults).workspace_profile),
        ...known.defaults.workspace_profile,
      },
      agent_profile: {
        ...record(record(raw.defaults).agent_profile),
        ...known.defaults.agent_profile,
      },
    },
    inactivity: { ...record(raw.inactivity), ...known.inactivity },
    retention: { ...record(raw.retention), ...known.retention },
    limits: { ...record(raw.limits), ...known.limits },
    triggers: { ...record(raw.triggers), ...known.triggers },
    marketplace: { ...record(raw.marketplace), ...known.marketplace },
  };
}
export const storedPlatformSettings = object({
  setting_changes: optional(
    dictionary(
      object({
        actor_reference: text,
        actor_name: optional(text),
        at: text,
        revision: integer,
      }),
    ),
  ),
  revision: integer,
  settings: platformSettings,
  model_budget_writes_available: optional(boolean),
  model_request_budget_writes_available: optional(boolean),
  platform_spend_writes_available: optional(boolean),
});
export type StoredPlatformSettings = Output<typeof storedPlatformSettings>;
export const replaceSettingsRequest = object({
  expected_revision: integer,
  settings: platformSettings,
});
export type ReplaceSettingsRequest = Output<typeof replaceSettingsRequest>;

const patchFields = object({
  usage_reporting: optional(sections.usage_reporting),
  expected_revision: integer,
  defaults: optional(sections.defaults),
  inactivity: optional(sections.inactivity),
  retention: optional(sections.retention),
  limits: optional(sections.limits),
  triggers: optional(sections.triggers),
  marketplace: optional(sections.marketplace),
});
/** Supplied sections replace a whole section; omitted sections remain unchanged. */
export type PatchSettingsRequest = Output<typeof patchFields>;
export function patchSettingsRequest(value: unknown): PatchSettingsRequest {
  const raw = record(value);
  if (Object.keys(raw).some((key) => key !== 'expected_revision' && !Object.hasOwn(sections, key)))
    throw new ProtocolError('unknown platform settings section');
  const parsed = patchFields(value);
  if (Object.keys(parsed).length < 2)
    throw new ProtocolError('at least one settings section is required');
  return parsed;
}
export const resetSettingsRequest = object({ expected_revision: integer });
export type ResetSettingsRequest = Output<typeof resetSettingsRequest>;
