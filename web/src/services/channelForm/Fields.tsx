import { useId, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { useValidationPath } from './ValidationPath';
import { text } from './model';
import '../NodeInspector.css';
import { ConfigurationDepth, useConfigurationSection } from '../ConfigurationSections';
export function Section({
  path,
  title,
  description,
  children,
  navigation,
}: {
  path?: string;
  title: string;
  description?: string;
  children: ReactNode;
  navigation?: boolean;
}) {
  const validationPath = useValidationPath(path);
  const { id, ref, navigable, depth } = useConfigurationSection(title, navigation);
  return (
    <fieldset
      data-validation-path={validationPath}
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
  path,
  label,
  value,
  onChange,
  readOnly,
  required,
  secret,
  multiline,
  hint,
}: {
  path?: string;
  label: string;
  value: unknown;
  required?: boolean;
  secret?: boolean;
  multiline?: boolean;
  hint?: string;
} & (
  { readOnly: true; onChange?: never } | { readOnly?: false; onChange: (value: string) => void }
)) {
  const validationPath = useValidationPath(path);
  const id = useId();
  const description = [required ? 'Required.' : undefined, hint].filter(Boolean).join(' ');
  return (
    <div className="channel-value-field" data-validation-path={validationPath}>
      <label htmlFor={id}>{label}</label>
      {multiline ? (
        <textarea
          id={id}
          aria-describedby={description ? `${id}-hint` : undefined}
          value={text(value)}
          onChange={onChange ? (e) => onChange(e.target.value) : undefined}
          readOnly={readOnly}
          rows={3}
          maxLength={32768}
          required={required}
        />
      ) : (
        <Input
          id={id}
          aria-describedby={description ? `${id}-hint` : undefined}
          value={text(value)}
          onChange={onChange ? (e) => onChange(e.target.value) : undefined}
          readOnly={readOnly}
          type={secret ? 'password' : 'text'}
          autoComplete={secret ? 'new-password' : 'off'}
          required={required}
        />
      )}
      {description ? (
        <p id={`${id}-hint`} className="channel-field-hint">
          {description}
        </p>
      ) : null}
    </div>
  );
}
export function NumberField({
  path,
  label,
  value,
  onChange,
  min = 0,
  max,
  step = 1,
}: {
  path?: string;
  label: string;
  value: unknown;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
  step?: number | 'any';
}) {
  const validationPath = useValidationPath(path);
  const id = useId();
  return (
    <div className="channel-value-field" data-validation-path={validationPath}>
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
  path,
  label,
  value,
  options,
  onChange,
}: {
  path?: string;
  label: string;
  value: unknown;
  options: readonly (string | { value: string; label: string; unavailable?: string })[];
  onChange: (v: string) => void;
}) {
  const validationPath = useValidationPath(path);
  const id = useId();
  const entries = options.map((v) =>
    typeof v === 'string' ? { value: v, label: v.replaceAll('_', ' ') } : v,
  );
  const current = text(value);
  if (current && !entries.some((v) => v.value === current))
    entries.push({ value: current, label: current });
  return (
    <div className="channel-value-field" data-validation-path={validationPath}>
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
  path,
  label,
  value,
  onChange,
  max = 64,
  requiredItems = false,
}: {
  path?: string;
  label: string;
  value: readonly string[];
  onChange: (v: string[]) => void;
  max?: number;
  requiredItems?: boolean;
}) {
  return (
    <Section title={label} path={path}>
      {value.map((item, index) => (
        <div className="channel-form-row" key={index}>
          <TextField
            path={path ? `${path}/${index}` : undefined}
            required={requiredItems}
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
