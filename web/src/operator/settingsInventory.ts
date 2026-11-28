import type { PlatformSettings } from '@cma/client';
import { cloudAmounts, ownerAmounts, platformAmounts, spendBudget } from './spendBudget';

type Value = string | number;
type Fields = readonly (readonly [string, Value])[];
export interface Setting {
  readonly id: string;
  readonly title: string;
  readonly value: Value;
  readonly description: string;
  readonly details?: string;
  readonly fields?: Fields;
  readonly items?: readonly { label: string; fields: Fields }[];
  readonly readOnly?: string;
}

const state = (enabled: boolean) => (enabled ? 'Enabled' : 'Disabled');
const reference = ({ id, revision }: { id: string; revision: number }) =>
  id + ' · revision ' + revision;

function amounts<K extends string>(
  values: Record<NoInfer<K>, Value>,
  labels: Record<K, string>,
): Fields {
  return (Object.keys(labels) as K[]).map((key) => [labels[key], values[key]]);
}

export function settingsInventory(settings: PlatformSettings): readonly Setting[] {
  const budget = spendBudget(settings.model_budget);
  const platform = budget?.platform;
  const cloud = platform?.cloud;
  const policy = budget?.default_policy;
  const models = budget?.request_models ?? [];
  const overrides = budget?.owners ?? [];
  const unavailable = 'Unavailable';
  const unset = 'Not configured';
  const historical = 'Retained for compatibility. This page does not edit legacy defaults.';
  return [
    {
      id: 'spend_controls',
      title: 'Spend controls',
      value: budget ? state(budget.enabled) : unavailable,
      description: 'Controls accounting for new model spend admissions.',
      details:
        'Disabling retains policies, reservations and existing stops. Amounts use exact accounting units. Currency and conversion metadata are unavailable. Strict enforcement for runtime model calls requires the model gateway.',
    },
    {
      id: 'platform_period',
      title: 'Platform period',
      value: !budget
        ? unavailable
        : platform
          ? String(platform.period_cap) + ' accounting units'
          : unset,
      description: 'Sets the spend cap and thresholds for a reporting period.',
      fields: platform && [
        ...amounts(platform, platformAmounts),
        ['Period start (inclusive)', platform.period_start],
        ['Period end (exclusive)', platform.period_end],
      ],
    },
    {
      id: 'cloud_metering',
      title: 'Cloud metering',
      value: !budget
        ? unavailable
        : cloud
          ? String(cloud.run_rate_units_per_day) + ' accounting units/day'
          : unset,
      description: 'Projects daily cloud spend between observations.',
      details:
        'Missing or stale observations block new spend commitments when metering is configured.',
      fields: cloud ? amounts(cloud, cloudAmounts) : undefined,
    },
    {
      id: 'default_owner_policy',
      title: 'Default owner policy',
      value: !budget
        ? unavailable
        : policy
          ? String(policy.owner_day_limit) + ' accounting units/day'
          : unset,
      description: 'Applies to owners without an explicit override.',
      details:
        'The configured timezone defines each day. Changing timezone or token valuation can require usage reconciliation.',
      fields: policy && [...amounts(policy, ownerAmounts), ['Day timezone', policy.day_timezone]],
    },
    {
      id: 'owner_overrides',
      title: 'Owner overrides',
      value: budget
        ? overrides.length === 0
          ? 'None'
          : overrides.length + ' configured'
        : unavailable,
      description: 'Applies a separate spend policy to each listed owner.',
      details:
        'Removing an override returns the owner to the default policy, when configured. Existing reservations are retained.',
      items: overrides.map(({ owner, policy }) => ({
        label: owner,
        fields: [...amounts(policy, ownerAmounts), ['Day timezone', policy.day_timezone]],
      })),
    },
    {
      id: 'request_models',
      title: 'Request models',
      value: budget ? (models.length === 0 ? 'None' : models.length + ' configured') : unavailable,
      description: 'Lists model names and input framing overhead.',
      details:
        'Owner token valuations must cover the listed models. An approved model still needs provider usage finality.',
      items: models.map(({ model, input_overhead_tokens }) => ({
        label: model,
        fields: [['Input overhead (tokens)', input_overhead_tokens]],
      })),
    },
    {
      id: 'sandbox_profile',
      title: 'Default Sandbox Profile',
      value: reference(settings.defaults.sandbox_profile),
      description: 'Pinned Sandbox Profile for historical default-based workflows.',
      readOnly: historical,
    },
    {
      id: 'workspace_profile',
      title: 'Default Workspace Profile',
      value: reference(settings.defaults.workspace_profile),
      description: 'Pinned Workspace Profile for historical default-based workflows.',
      readOnly: historical,
    },
    {
      id: 'agent_profile',
      title: 'Default Agent Profile',
      value: reference(settings.defaults.agent_profile),
      description: 'Pinned Agent Profile for historical default-based workflows.',
      readOnly: historical,
    },
    {
      id: 'auto_pause',
      title: 'Automatic pause',
      value: state(settings.inactivity.auto_pause_enabled === true),
      description: 'Pauses eligible Sandboxes after their Agents are idle.',
    },
    {
      id: 'idle_threshold',
      title: 'Idle threshold',
      value: String(settings.inactivity.pause_after_seconds) + ' seconds',
      description: 'Time a Sandbox must remain idle before it can be paused.',
    },
    {
      id: 'measured_usage',
      title: 'Measured usage',
      value: settings.usage_reporting?.measured_visible ? 'Shown' : 'Hidden',
      description: 'Shows measured CPU, memory and disk alongside allocated resources.',
    },
    {
      id: 'agent_log_retention',
      title: 'Agent log retention',
      value: String(settings.retention.agent_log_days) + ' days',
      description: 'Number of days the platform retains Agent logs.',
    },
    {
      id: 'sandboxes_per_user',
      title: 'Sandboxes per user',
      value: settings.limits.max_sandboxes_per_user,
      description: 'Maximum number of Sandboxes per user.',
    },
    {
      id: 'visitor_sandboxes',
      title: 'Visitor Sandboxes',
      value: settings.limits.visitor_max_sandboxes,
      description: 'Maximum number of Sandboxes per visitor.',
    },
    {
      id: 'visitor_launches',
      title: 'Visitor launches per day',
      value: settings.limits.visitor_max_launches_per_day,
      description: 'Maximum daily Trigger launches per visitor.',
    },
    {
      id: 'triggers_enabled',
      title: 'Triggers',
      value: state(settings.triggers.enabled),
      description: 'Controls whether visitors can use CMA Triggers.',
    },
    {
      id: 'trigger_orchestration',
      title: 'Trigger orchestration',
      value: settings.triggers.orchestration_paused ? 'Paused' : 'Running',
      description: 'Controls whether admission processing is paused.',
    },
    {
      id: 'marketplace',
      title: 'Marketplace',
      value: state(settings.marketplace.enabled),
      description: 'Effective availability of the legacy Profile marketplace.',
      readOnly: 'The Marketplace setting is read-only on this page.',
    },
  ];
}
