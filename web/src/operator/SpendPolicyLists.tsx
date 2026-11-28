import { Button } from '../ui/Button';
import { OwnerPolicyFields } from './OwnerPolicyFields';
import { SpendField } from './SpendField';
import { blankOwner, exactAmount, type SpendBudget } from './spendBudget';
import type { SpendErrors } from './spendValidation';

type Props = {
  readonly budget: SpendBudget;
  readonly disabled: boolean;
  readonly requestWritesAvailable: boolean;
  readonly errors: SpendErrors;
  readonly onChange: (budget: SpendBudget) => void;
};

export function OwnerOverridesEditor({
  budget,
  disabled,
  errors,
  onChange,
}: Omit<Props, 'requestWritesAvailable'>) {
  return (
    <>
      {errors.owners ? (
        <p role="alert" className="operator-failure">
          {errors.owners}
        </p>
      ) : null}
      {budget.owners.map((entry, index) => {
        const path = `owners.${index}`;
        const replace = (updated: typeof entry) =>
          onChange({
            ...budget,
            owners: budget.owners.map((row, i) => (i === index ? updated : row)),
          });
        return (
          <div key={index} role="group" aria-label={`Owner override ${index + 1}`}>
            <h4>Owner override {index + 1}</h4>
            <SpendField
              label={`Owner override ${index + 1}: owner ID`}
              value={entry.owner}
              disabled={disabled}
              error={errors[`${path}.owner`]}
              onChange={(owner) => replace({ ...entry, owner })}
            />
            <OwnerPolicyFields
              value={entry.policy}
              path={`${path}.policy`}
              labelPrefix={`Owner override ${index + 1}: `}
              disabled={disabled}
              errors={errors}
              onChange={(policy) => replace({ ...entry, policy })}
            />
            <Button
              disabled={disabled}
              onClick={() =>
                onChange({ ...budget, owners: budget.owners.filter((_, i) => i !== index) })
              }
            >
              Remove owner override {index + 1}
            </Button>
          </div>
        );
      })}
      <Button
        disabled={disabled || budget.owners.length >= 1024}
        onClick={() =>
          onChange({ ...budget, owners: [...budget.owners, { owner: '', policy: blankOwner() }] })
        }
      >
        Add owner override
      </Button>
    </>
  );
}

export function RequestModelsEditor({
  budget,
  disabled,
  requestWritesAvailable,
  errors,
  onChange,
}: Props) {
  const models = budget.request_models ?? [];
  const requestDisabled = disabled || !requestWritesAvailable;
  return (
    <>
      {errors.request_models ? (
        <p role="alert" className="operator-failure">
          {errors.request_models}
        </p>
      ) : null}
      {models.map((entry, index) => {
        const path = `request_models.${index}`;
        const replace = (updated: typeof entry) =>
          onChange({
            ...budget,
            request_models: models.map((row, i) => (i === index ? updated : row)),
          });
        return (
          <div key={index} role="group" aria-label={`Request model ${index + 1}`}>
            <div className="operator-settings-grid">
              <SpendField
                label={`Request model ${index + 1}: model name`}
                value={entry.model}
                disabled={requestDisabled}
                error={errors[`${path}.model`]}
                onChange={(model) => replace({ ...entry, model })}
              />
              <SpendField
                label={`Request model ${index + 1}: input overhead (tokens)`}
                value={entry.input_overhead_tokens}
                disabled={requestDisabled}
                error={errors[`${path}.input_overhead_tokens`]}
                numeric
                onChange={(text) => replace({ ...entry, input_overhead_tokens: exactAmount(text) })}
              />
            </div>
            <Button
              disabled={requestDisabled}
              onClick={() =>
                onChange({ ...budget, request_models: models.filter((_, i) => i !== index) })
              }
            >
              Remove request model {index + 1}
            </Button>
          </div>
        );
      })}
      <Button
        disabled={requestDisabled || models.length >= 64}
        onClick={() =>
          onChange({
            ...budget,
            request_models: [...models, { model: '', input_overhead_tokens: '' }],
          })
        }
      >
        Add request model
      </Button>
    </>
  );
}
