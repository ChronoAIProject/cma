import { useId } from 'react';

export function ChoiceGroup<Value extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  describedBy,
}: {
  readonly label: string;
  readonly value: Value;
  readonly options: readonly { readonly value: Value; readonly label: string }[];
  readonly onChange: (value: Value) => void;
  readonly disabled?: boolean;
  readonly describedBy?: string;
}) {
  const name = useId();
  return (
    <div
      className="profile-choices"
      role="radiogroup"
      aria-label={label}
      aria-describedby={describedBy}
    >
      {options.map((option) => (
        <label key={option.value} className="profile-choices__option">
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={option.value === value}
            disabled={disabled}
            onChange={() => onChange(option.value)}
          />
          <span>{option.label}</span>
        </label>
      ))}
    </div>
  );
}
