import { useId } from 'react';
import { Button } from '../ui/Button';
import { SpendField } from './SpendField';
import { OwnerPolicyFields } from './OwnerPolicyFields';
import { SpendPolicyLists } from './SpendPolicyLists';
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

export function SpendControls({
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
}) {
  const budget = spendBudget(value);
  const id = useId();
  return (
    <section aria-labelledby={`${id}-heading`}>
      <h3 id={`${id}-heading`}>Spend controls</h3>
      {!budget ? (
        <p className="operator-notice">
          Not yet available: this server does not expose the supported spend-controls configuration.
        </p>
      ) : (
        <>
          {!writesAvailable ? (
            <p role="status" className="operator-notice">
              Saving spend controls is not available yet. Current values remain readable.
            </p>
          ) : null}
          <p className="operator-notice">
            Amounts use exact accounting units from the platform policy. Currency and conversion
            metadata are not yet available.
          </p>
          <label className="operator-switch">
            <input
              type="checkbox"
              checked={budget.enabled}
              disabled={disabled}
              aria-describedby={`${id}-scope`}
              onChange={(event) => onChange({ ...budget, enabled: event.target.checked })}
            />
            Enable spend controls
          </label>
          <p id={`${id}-scope`} className="operator-notice">
            Disabling stops new admission accounting and retains configured policies. Existing
            reservations and stops remain effective. Configuring cloud metering enables platform
            warning and hard-stop threshold actions. Strict provider-spend enforcement for runtime
            model calls still requires the model gateway.
          </p>
          {errors.enabled ? (
            <p role="alert" className="operator-failure">
              {errors.enabled}
            </p>
          ) : null}
          <h4>Platform period</h4>
          {budget.platform ? (
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
          )}
          {budget.platform ? (
            <>
              <h4>Cloud metering</h4>
              <p className="operator-notice">
                Set both values to configure cloud metering. Missing or stale cloud observations
                make metering unavailable and block new spend commitments.
              </p>
              {!platformWritesAvailable ? (
                <p role="status" className="operator-notice">
                  Saving cloud metering is not available yet. Current values remain readable.
                </p>
              ) : null}
              {budget.platform.cloud ? (
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
              )}
            </>
          ) : null}
          <h4>Default owner policy</h4>
          <p className="operator-notice">
            Applies to owners without an explicit override. {budget.owners.length} explicit owner
            overrides are configured below. Owners without either policy remain outside the enabled
            scope. Remove the default policy to limit enforcement to listed owners.
          </p>
          {budget.default_policy ? (
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
          )}
          <SpendPolicyLists
            budget={budget}
            disabled={disabled}
            requestWritesAvailable={requestWritesAvailable}
            errors={errors}
            onChange={onChange}
          />
          <p className="operator-notice">
            Daily caps use the configured timezone. Changing timezone or token valuation may require
            reconciliation of outstanding usage before the server accepts the change.
          </p>
        </>
      )}
    </section>
  );
}
