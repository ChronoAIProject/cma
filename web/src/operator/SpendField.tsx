import { useId } from 'react';
import { Input } from '../ui/Input';
import type { Amount } from './spendBudget';

export function SpendField({
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
