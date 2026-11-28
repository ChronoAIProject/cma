import type { ReactNode } from 'react';
import type { PlatformSettings } from '@cma/client';
import { IdleThresholdFields, NumberField } from './SettingsFields';

function toggle(
  label: string,
  checked: boolean,
  disabled: boolean,
  onChange: (value: boolean) => void,
) {
  return (
    <label className="operator-switch">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      {label}
    </label>
  );
}

export function settingEditors(
  draft: PlatformSettings,
  disabled: boolean,
  fieldError: (path: string) => string | undefined,
  update: (draft: PlatformSettings) => void,
): Readonly<Record<string, ReactNode>> {
  const autoPause = draft.inactivity.auto_pause_enabled === true;
  const setThreshold = (pause_after_seconds: number) =>
    update({ ...draft, inactivity: { ...draft.inactivity, pause_after_seconds } });
  const limit = (label: string, field: keyof PlatformSettings['limits']) => (
    <NumberField
      label={label}
      value={draft.limits[field]}
      disabled={disabled}
      error={fieldError('limits.' + field)}
      onChange={(value) => update({ ...draft, limits: { ...draft.limits, [field]: value } })}
    />
  );
  return {
    auto_pause: toggle('Enable auto pause', autoPause, disabled, (auto_pause_enabled) =>
      update({ ...draft, inactivity: { ...draft.inactivity, auto_pause_enabled } }),
    ),
    idle_threshold: autoPause ? (
      <IdleThresholdFields
        seconds={draft.inactivity.pause_after_seconds}
        disabled={disabled}
        onChange={setThreshold}
      />
    ) : (
      <NumberField
        label="Pause after (seconds)"
        value={draft.inactivity.pause_after_seconds}
        minimum={60}
        maximum={86400}
        disabled={disabled}
        error={fieldError('inactivity.pause_after_seconds')}
        onChange={setThreshold}
      />
    ),
    measured_usage: toggle(
      'Show measured usage',
      draft.usage_reporting?.measured_visible === true,
      disabled,
      (measured_visible) =>
        update({ ...draft, usage_reporting: { ...draft.usage_reporting, measured_visible } }),
    ),
    agent_log_retention: (
      <NumberField
        label="Agent log retention (days)"
        value={draft.retention.agent_log_days}
        error={fieldError('retention.agent_log_days')}
        disabled={disabled}
        onChange={(agent_log_days) =>
          update({ ...draft, retention: { ...draft.retention, agent_log_days } })
        }
      />
    ),
    sandboxes_per_user: limit('Sandboxes per user', 'max_sandboxes_per_user'),
    visitor_sandboxes: limit('Visitor Sandboxes', 'visitor_max_sandboxes'),
    visitor_launches: limit('Visitor launches per day', 'visitor_max_launches_per_day'),
    triggers_enabled: toggle('Triggers enabled', draft.triggers.enabled, disabled, (enabled) =>
      update({ ...draft, triggers: { ...draft.triggers, enabled } }),
    ),
    trigger_orchestration: toggle(
      'Orchestration paused',
      draft.triggers.orchestration_paused,
      disabled,
      (orchestration_paused) =>
        update({ ...draft, triggers: { ...draft.triggers, orchestration_paused } }),
    ),
  };
}
