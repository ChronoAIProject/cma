import type { ReactNode } from 'react';
import { Button } from '../ui/Button';
import { SpendField } from './SpendField';
import { OwnerPolicyFields } from './OwnerPolicyFields';
import { OwnerOverridesEditor, RequestModelsEditor } from './SpendPolicyLists';
import {
  blankOwner,
  blankPlatform,
  blankCloud,
  cloudAmounts,
  exactAmount,
  platformAmounts,
  spendBudget,
  type SpendBudget,
} from './spendBudget';
import type { SpendErrors } from './spendValidation';

export function spendEditors({
  value,
  disabled,
  writesAvailable,
  requestWritesAvailable,
  platformWritesAvailable,
  errors,
  onChange,
}: {
  readonly value: unknown;
  readonly disabled: boolean;
  readonly writesAvailable: boolean;
  readonly requestWritesAvailable: boolean;
  readonly platformWritesAvailable: boolean;
  readonly errors: SpendErrors;
  readonly onChange: (budget: SpendBudget) => void;
}): Record<string, { editor?: ReactNode; readOnly?: string }> {
  const budget = spendBudget(value);
  const unavailable = 'This server does not expose the supported spend-controls configuration.';
  const readOnly = writesAvailable
    ? undefined
    : 'Saving spend controls is not available yet. Current values remain readable.';
  if (!budget)
    return Object.fromEntries(
      [
        'spend_controls',
        'platform_period',
        'cloud_metering',
        'default_owner_policy',
        'owner_overrides',
        'request_models',
      ].map((key) => [key, { readOnly: unavailable }]),
    );

  return {
    spend_controls: {
      readOnly,
      editor: (
        <>
          <label className="operator-switch">
            <input
              type="checkbox"
              checked={budget.enabled}
              disabled={disabled}
              onChange={(event) => onChange({ ...budget, enabled: event.target.checked })}
            />
            Enable spend controls
          </label>
          {errors.enabled ? (
            <p role="alert" className="operator-failure">
              {errors.enabled}
            </p>
          ) : null}
        </>
      ),
    },
    platform_period: {
      readOnly,
      editor: budget.platform ? (
        <div className="operator-settings-grid">
          {Object.entries(platformAmounts).map(([field, label]) => {
            const key = field as keyof typeof platformAmounts;
            return (
              <SpendField
                key={key}
                label={label}
                value={budget.platform![key]}
                disabled={disabled}
                error={errors[`platform.${key}`]}
                numeric
                onChange={(value) =>
                  onChange({
                    ...budget,
                    platform: { ...budget.platform!, [key]: exactAmount(value) },
                  })
                }
              />
            );
          })}
          {(['period_start', 'period_end'] as const).map((key) => (
            <SpendField
              key={key}
              label={
                key === 'period_start'
                  ? 'Period start (inclusive, RFC3339)'
                  : 'Period end (exclusive, RFC3339)'
              }
              value={budget.platform![key]}
              disabled={disabled}
              error={errors[`platform.${key}`]}
              onChange={(value) =>
                onChange({ ...budget, platform: { ...budget.platform!, [key]: value } })
              }
            />
          ))}
        </div>
      ) : (
        <Button
          disabled={disabled}
          onClick={() => onChange({ ...budget, platform: blankPlatform() })}
        >
          Configure platform period
        </Button>
      ),
    },
    cloud_metering: {
      readOnly:
        readOnly ??
        (!platformWritesAvailable
          ? 'Saving cloud metering is not available yet. Current values remain readable.'
          : !budget.platform
            ? 'Configure and save the platform period first.'
            : undefined),
      editor:
        budget.platform &&
        (budget.platform.cloud ? (
          <>
            <div className="operator-settings-grid">
              {Object.entries(cloudAmounts).map(([field, label]) => {
                const key = field as keyof typeof cloudAmounts;
                return (
                  <SpendField
                    key={key}
                    label={label}
                    value={budget.platform!.cloud![key]}
                    disabled={disabled || !platformWritesAvailable}
                    error={errors[`platform.cloud.${key}`]}
                    numeric
                    onChange={(value) =>
                      onChange({
                        ...budget,
                        platform: {
                          ...budget.platform!,
                          cloud: { ...budget.platform!.cloud!, [key]: exactAmount(value) },
                        },
                      })
                    }
                  />
                );
              })}
            </div>
            <Button
              disabled={disabled || !platformWritesAvailable}
              onClick={() =>
                onChange({ ...budget, platform: { ...budget.platform!, cloud: undefined } })
              }
            >
              Remove cloud metering
            </Button>
          </>
        ) : (
          <Button
            disabled={disabled || !platformWritesAvailable}
            onClick={() =>
              onChange({ ...budget, platform: { ...budget.platform!, cloud: blankCloud() } })
            }
          >
            Configure cloud metering
          </Button>
        )),
    },
    default_owner_policy: {
      readOnly,
      editor: budget.default_policy ? (
        <>
          <OwnerPolicyFields
            value={budget.default_policy}
            path="default_policy"
            disabled={disabled}
            errors={errors}
            onChange={(default_policy) => onChange({ ...budget, default_policy })}
          />
          <Button
            disabled={disabled}
            onClick={() => onChange({ ...budget, default_policy: undefined })}
          >
            Remove default owner policy
          </Button>
        </>
      ) : (
        <Button
          disabled={disabled}
          onClick={() => onChange({ ...budget, default_policy: blankOwner() })}
        >
          Configure default owner policy
        </Button>
      ),
    },
    owner_overrides: {
      readOnly,
      editor: (
        <OwnerOverridesEditor
          budget={budget}
          disabled={disabled}
          errors={errors}
          onChange={onChange}
        />
      ),
    },
    request_models: {
      readOnly:
        readOnly ??
        (!requestWritesAvailable
          ? 'Saving request models is not available yet. Current models remain readable.'
          : undefined),
      editor: (
        <RequestModelsEditor
          budget={budget}
          disabled={disabled}
          requestWritesAvailable={requestWritesAvailable}
          errors={errors}
          onChange={onChange}
        />
      ),
    },
  };
}
