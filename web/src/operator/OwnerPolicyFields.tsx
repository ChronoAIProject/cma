import { SpendField } from './SpendField';
import { exactAmount, ownerAmounts, type OwnerBudget } from './spendBudget';
import type { SpendErrors } from './spendValidation';

export function OwnerPolicyFields({
  value,
  path,
  labelPrefix = '',
  disabled,
  errors,
  onChange,
}: {
  readonly value: OwnerBudget;
  readonly path: string;
  readonly labelPrefix?: string;
  readonly disabled: boolean;
  readonly errors: SpendErrors;
  readonly onChange: (policy: OwnerBudget) => void;
}) {
  return (
    <div className="operator-settings-grid">
      {Object.entries(ownerAmounts).map(([field, label]) => {
        const key = field as keyof typeof ownerAmounts;
        return (
          <SpendField
            key={key}
            label={labelPrefix + label}
            value={value[key]}
            disabled={disabled}
            error={errors[`${path}.${key}`]}
            numeric
            onChange={(text) => onChange({ ...value, [key]: exactAmount(text) })}
          />
        );
      })}
      <SpendField
        label={labelPrefix + 'Day-boundary timezone (IANA)'}
        value={value.day_timezone}
        disabled={disabled}
        error={errors[`${path}.day_timezone`]}
        onChange={(day_timezone) => onChange({ ...value, day_timezone })}
      />
    </div>
  );
}
