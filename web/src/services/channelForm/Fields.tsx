import { useId, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { text } from './model';
import '../NodeInspector.css';
import { ConfigurationDepth, useConfigurationSection } from '../ConfigurationSections';
export function Section({
  title,
  description,
  children,
  navigation,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  navigation?: boolean;
}) {
  const { id, ref, navigable, depth } = useConfigurationSection(title, navigation);
  return (
    <fieldset
      className="channel-form-section"
      id={id}
      ref={ref}
      tabIndex={navigable ? -1 : undefined}
      data-config-section={navigable || undefined}
    >
      <legend>{title}</legend>
      {description ? <p className="channel-section-description">{description}</p> : null}
      <ConfigurationDepth value={depth}>{children}</ConfigurationDepth>
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
          aria-describedby={hint ? `${id}-hint` : undefined}
          value={text(value)}
          onChange={(e) => onChange(e.target.value)}
          rows={3}
          maxLength={32768}
          required={required}
        />
      ) : (
        <Input
          id={id}
          aria-describedby={hint ? `${id}-hint` : undefined}
          value={text(value)}
          onChange={(e) => onChange(e.target.value)}
          type={secret ? 'password' : 'text'}
          autoComplete={secret ? 'new-password' : 'off'}
          required={required}
        />
      )}
      {hint ? (
        <p id={`${id}-hint`} className="channel-field-hint">
          {hint}
        </p>
      ) : null}
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
  hint,
}: {
  label: string;
  value: unknown;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  hint?: string;
}) {
  return (
    <label className="channel-toggle-field">
      <input
        type="checkbox"
        checked={value === true}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span>{label}</span>
        {hint ? <small>{hint}</small> : null}
      </span>
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
