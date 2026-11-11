import { useId } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  blankOwner,
  blankPlatform,
  exactAmount,
  ownerAmounts,
  platformAmounts,
  spendBudget,
  type Amount,
  type SpendBudget,
} from './spendBudget';
import type { SpendErrors } from './spendValidation';

export function SpendControls({
  value,
  disabled,
  writesAvailable,
  errors,
  onChange,
}: {
  readonly value: unknown;
  readonly disabled: boolean;
  readonly writesAvailable: boolean;
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
            reservations and stops remain effective. These settings do not yet establish a strict
            provider-spend ceiling; platform threshold actions and model-call enforcement require
            their backend integrations.
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
          <h4>Default owner policy</h4>
          <p className="operator-notice">
            Applies to owners without an explicit override. {budget.owners.length} explicit owner
            overrides are retained unchanged. Owners without either policy remain outside the
            enabled scope.
          </p>
          {budget.default_policy ? (
            <div className="operator-settings-grid">
              {Object.entries(ownerAmounts).map(([field, label]) => {
                const key = field as keyof typeof ownerAmounts;
                return (
                  <SpendField
                    key={key}
                    label={label}
                    value={budget.default_policy![key]}
                    disabled={disabled}
                    error={errors[`default_policy.${key}`]}
                    numeric
                    onChange={(value) =>
                      onChange({
                        ...budget,
                        default_policy: { ...budget.default_policy!, [key]: exactAmount(value) },
                      })
                    }
                  />
                );
              })}
              <SpendField
                label="Day-boundary timezone (IANA)"
                value={budget.default_policy.day_timezone}
                disabled={disabled}
                error={errors['default_policy.day_timezone']}
                onChange={(value) =>
                  onChange({
                    ...budget,
                    default_policy: { ...budget.default_policy!, day_timezone: value },
                  })
                }
              />
            </div>
          ) : (
            <Button
              disabled={disabled}
              onClick={() => onChange({ ...budget, default_policy: blankOwner() })}
            >
              Configure default owner policy
            </Button>
          )}
          <p className="operator-notice">
            Daily caps use the configured timezone. Changing timezone or token valuation may require
            reconciliation of outstanding usage before the server accepts the change.
          </p>
        </>
      )}
    </section>
  );
}

function SpendField({
  label,
  value,
  disabled,
  error,
  numeric = false,
  onChange,
}: {
  readonly label: string;
  readonly value: Amount;
  readonly disabled: boolean;
  readonly error?: string;
  readonly numeric?: boolean;
  readonly onChange: (value: string) => void;
}) {
  const id = useId();
  return (
    <label className="operator-field">
      <span>{label}</span>
      <Input
        aria-label={label}
        type="text"
        inputMode={numeric ? 'numeric' : undefined}
        value={value}
        disabled={disabled}
        required
        aria-invalid={!!error}
        aria-describedby={error ? id : undefined}
        onChange={(event) => onChange(event.target.value)}
      />
      {error ? (
        <span id={id} className="operator-failure">
          {error}
        </span>
      ) : null}
    </label>
  );
}
