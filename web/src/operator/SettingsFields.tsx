import { ValidatedNumberInput } from './ValidatedNumberInput';

export function IdleThresholdFields({
  seconds,
  disabled,
  onChange,
}: {
  readonly seconds: number;
  readonly disabled: boolean;
  readonly onChange: (seconds: number) => void;
}) {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const hours = Math.floor(safeSeconds / 3600);
  const minutes = Math.floor((safeSeconds % 3600) / 60);
  const remainder = safeSeconds % 60;
  const change = (part: 'hours' | 'minutes' | 'seconds', value: number) => {
    const next =
      part === 'hours'
        ? value * 3600 + minutes * 60 + remainder
        : part === 'minutes'
          ? hours * 3600 + value * 60 + remainder
          : hours * 3600 + minutes * 60 + value;
    onChange(next);
  };
  return (
    <div className="operator-settings-grid" aria-label="Automatic pause idle threshold">
      <NumberField
        label="Pause after hours"
        value={hours}
        minimum={0}
        maximum={24}
        disabled={disabled}
        onChange={(value) => change('hours', value)}
      />
      <NumberField
        label="Pause after minutes"
        value={minutes}
        minimum={0}
        maximum={59}
        disabled={disabled}
        onChange={(value) => change('minutes', value)}
      />
      <NumberField
        label="Pause after seconds"
        value={remainder}
        minimum={0}
        maximum={59}
        disabled={disabled}
        onChange={(value) => change('seconds', value)}
      />
      <p className="operator-notice">Choose at least 60 seconds and at most 24 hours.</p>
    </div>
  );
}

export function NumberField({
  label,
  value,
  minimum = 0,
  maximum = 4_294_967_295,
  step = 1,
  disabled,
  error,
  onChange,
}: {
  readonly label: string;
  readonly value: number;
  readonly minimum?: number;
  readonly maximum?: number;
  readonly step?: number | 'any';
  readonly disabled: boolean;
  readonly error?: string;
  readonly onChange: (value: number) => void;
}) {
  return (
    <label className="operator-field">
      <span>{label}</span>
      <ValidatedNumberInput
        aria-label={label}
        value={Number.isNaN(value) ? '' : value}
        min={minimum}
        max={maximum}
        step={step}
        required
        disabled={disabled}
        error={error}
        onChange={(event) =>
          onChange(event.target.value === '' ? Number.NaN : Number(event.target.value))
        }
      />
    </label>
  );
}
