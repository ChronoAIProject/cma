import { ValidatedNumberInput } from './ValidatedNumberInput';

export function WarmPoolNumber({
  label,
  value,
  onChange,
  disabled,
  minimum = 0,
  maximum = 1000,
}: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly disabled: boolean;
  readonly minimum?: number;
  readonly maximum?: number;
}) {
  return (
    <label className="operator-field">
      <span>{label}</span>
      <ValidatedNumberInput
        aria-label={label}
        required
        min={minimum}
        max={maximum}
        step={1}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

export function validPoolNumber(value: string, minimum = 0, maximum = 1000): boolean {
  return (
    value.trim() !== '' &&
    Number.isInteger(Number(value)) &&
    Number(value) >= minimum &&
    Number(value) <= maximum
  );
}
