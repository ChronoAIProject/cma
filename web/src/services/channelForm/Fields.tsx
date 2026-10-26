import { useId, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { text } from './model';
export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="channel-form-section">
      <legend>{title}</legend>
      {children}
    </fieldset>
  );
}
export function TextField({
  label,
  value,
  onChange,
  required,
  secret,
  multiline,
  hint,
}: {
  label: string;
  value: unknown;
  onChange: (value: string) => void;
  required?: boolean;
  secret?: boolean;
  multiline?: boolean;
  hint?: string;
}) {
  const id = useId();
  return (
    <div className="channel-value-field">
      <label htmlFor={id}>{label}</label>
      {multiline ? (
        <textarea
          id={id}
          value={text(value)}
          onChange={(e) => onChange(e.target.value)}
          rows={3}
          maxLength={32768}
          required={required}
        />
      ) : (
        <Input
          id={id}
          value={text(value)}
          onChange={(e) => onChange(e.target.value)}
          type={secret ? 'password' : 'text'}
          autoComplete={secret ? 'new-password' : 'off'}
          required={required}
        />
      )}
      {hint ? <p className="channel-field-hint">{hint}</p> : null}
    </div>
  );
}
export function NumberField({
  label,
  value,
  onChange,
  min = 0,
  max,
  step = 1,
}: {
  label: string;
  value: unknown;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
  step?: number | 'any';
}) {
  const id = useId();
  return (
    <div className="channel-value-field">
      <label htmlFor={id}>{label}</label>
      <Input
        id={id}
        type="number"
        value={typeof value === 'number' && Number.isFinite(value) ? value : ''}
        min={min}
        max={max}
        step={step}
        onChange={(e) => onChange(e.target.valueAsNumber)}
        required
      />
    </div>
  );
}
export function Choice({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: unknown;
  options: readonly (string | { value: string; label: string; unavailable?: string })[];
  onChange: (v: string) => void;
}) {
  const id = useId();
  const entries = options.map((v) =>
    typeof v === 'string' ? { value: v, label: v.replaceAll('_', ' ') } : v,
  );
  const current = text(value);
  if (current && !entries.some((v) => v.value === current))
    entries.push({ value: current, label: current });
  return (
    <div className="channel-value-field">
      <label htmlFor={id}>{label}</label>
      <Picker id={id} label={label} value={current} options={entries} onChange={onChange} />
    </div>
  );
}
export function Toggle({
  label,
  value,
  onChange,
  disabled,
}: {
  label: string;
  value: unknown;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="channel-form-row">
      <input
        type="checkbox"
        checked={value === true}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}
export function StringList({
  label,
  value,
  onChange,
  max = 64,
}: {
  label: string;
  value: readonly string[];
  onChange: (v: string[]) => void;
  max?: number;
}) {
  return (
    <Section title={label}>
      {value.map((item, index) => (
        <div className="channel-form-row" key={index}>
          <TextField
            label={`${label} ${index + 1}`}
            value={item}
            onChange={(next) => onChange(value.map((v, i) => (i === index ? next : v)))}
          />
          <Button
            onClick={() => onChange(value.filter((_, i) => i !== index))}
            aria-label={`Remove ${label} ${index + 1}`}
          >
            Remove
          </Button>
        </div>
      ))}
      <Button disabled={value.length >= max} onClick={() => onChange([...value, ''])}>
        Add {label}
      </Button>
    </Section>
  );
}
