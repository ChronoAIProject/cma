import type { PlatformSettings } from '@chronoai/cma-sdk';

// Rebase only fields this editor changed; newer server fields and unedited values win.
export function reviewSettingsDraft(
  base: PlatformSettings,
  draft: PlatformSettings,
  current: PlatformSettings,
): PlatformSettings {
  const merge = <T extends object>(
    before: T,
    edited: T,
    latest: T,
    keys: readonly (keyof T)[],
  ): T => {
    const result = { ...latest };
    for (const key of keys) if (edited[key] !== before[key]) result[key] = edited[key];
    return result;
  };
  const reference = (kind: 'sandbox_profile' | 'workspace_profile' | 'agent_profile') => {
    const before = base.defaults[kind];
    const edited = draft.defaults[kind];
    return edited.id !== before.id || edited.revision !== before.revision
      ? { ...current.defaults[kind], id: edited.id, revision: edited.revision }
      : current.defaults[kind];
  };
  return {
    ...current,
    defaults: {
      ...current.defaults,
      sandbox_profile: reference('sandbox_profile'),
      workspace_profile: reference('workspace_profile'),
      agent_profile: reference('agent_profile'),
    },
    inactivity: merge(base.inactivity, draft.inactivity, current.inactivity, [
      'auto_pause_enabled',
      'pause_after_seconds',
      'grace_started_at',
    ]),
    ...(draft.usage_reporting?.measured_visible !== base.usage_reporting?.measured_visible
      ? {
          usage_reporting: {
            ...current.usage_reporting,
            measured_visible: draft.usage_reporting?.measured_visible ?? false,
          },
        }
      : {}),
    retention: merge(base.retention, draft.retention, current.retention, ['agent_log_days']),
    limits: merge(base.limits, draft.limits, current.limits, [
      'max_sandboxes_per_user',
      'visitor_max_sandboxes',
      'visitor_max_launches_per_day',
    ]),
    triggers: merge(base.triggers, draft.triggers, current.triggers, [
      'enabled',
      'orchestration_paused',
    ]),
    marketplace: merge(base.marketplace, draft.marketplace, current.marketplace, ['enabled']),
  };
}
