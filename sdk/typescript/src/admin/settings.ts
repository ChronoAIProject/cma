import { ProtocolError } from '../errors.js';
import { profileReference } from '../profiles/types.js';
import { boolean, integer, object, record, type Output } from '../schema.js';

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
const knownSettings = object({
  defaults: object({
    sandbox_profile: profileReference,
    workspace_profile: profileReference,
    agent_profile: profileReference,
  }),
  inactivity: object({ pause_after_seconds: pauseSeconds }),
  retention: object({ checkpoint_grace_days: u32, agent_log_days: u32 }),
  limits: object({
    max_sandboxes_per_user: u32,
    visitor_max_sandboxes: u32,
    visitor_max_launches_per_day: u32,
  }),
  triggers: object({ enabled: boolean, orchestration_paused: boolean }),
  marketplace: object({ enabled: boolean }),
});
type KnownSettings = Output<typeof knownSettings>;
export type PlatformSettings = KnownSettings & Readonly<Record<string, unknown>>;
export function platformSettings(value: unknown): PlatformSettings {
  const known = knownSettings(value);
  const raw = record(value);
  return {
    ...raw,
    ...known,
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
export const storedPlatformSettings = object({ revision: integer, settings: platformSettings });
export type StoredPlatformSettings = Output<typeof storedPlatformSettings>;
export const replaceSettingsRequest = object({
  expected_revision: integer,
  settings: platformSettings,
});
export type ReplaceSettingsRequest = Output<typeof replaceSettingsRequest>;
