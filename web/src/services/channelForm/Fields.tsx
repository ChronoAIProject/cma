import { useFieldValidity } from './useFieldValidity';
import { useId, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { useServerFieldError, useValidationPath } from './ValidationPath';
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
  validationMessage,
  pattern,
  minLength,
  maxLength,
}: {
  path?: string;
  label: string;
  value: unknown;
  required?: boolean;
  secret?: boolean;
  multiline?: boolean;
  hint?: string;
  validationMessage?: string;
  pattern?: string;
  minLength?: number;
  maxLength?: number;
} & (
  { readOnly: true; onChange?: never } | { readOnly?: false; onChange: (value: string) => void }
)) {
  const validationPath = useValidationPath(path);
  const id = useId();
  const serverError = useServerFieldError(validationPath);
  const message = validationMessage ?? serverError;
  const ref = useFieldValidity(message);
  const description = [required ? 'Required.' : undefined, hint].filter(Boolean).join(' ');
  return (
    <div className="channel-value-field" data-validation-path={validationPath} ref={ref}>
      <label htmlFor={id}>{label}</label>
      {multiline ? (
        <textarea
          id={id}
          aria-describedby={
            [description ? `${id}-hint` : '', message ? `${id}-error` : '']
              .filter(Boolean)
              .join(' ') || undefined
          }
          aria-invalid={message ? true : undefined}
          value={text(value)}
          onChange={onChange ? (e) => onChange(e.target.value) : undefined}
          readOnly={readOnly}
          rows={3}
          minLength={minLength}
          maxLength={maxLength ?? 32768}
          required={required}
        />
      ) : (
        <Input
          id={id}
          aria-describedby={
            [description ? `${id}-hint` : '', message ? `${id}-error` : '']
              .filter(Boolean)
              .join(' ') || undefined
          }
          aria-invalid={message ? true : undefined}
          value={text(value)}
          onChange={onChange ? (e) => onChange(e.target.value) : undefined}
          readOnly={readOnly}
          pattern={pattern}
          minLength={minLength}
          maxLength={maxLength}
          type={secret ? 'password' : 'text'}
          autoComplete={secret ? 'new-password' : 'off'}
          required={required}
        />
      )}
      {message ? (
        <p id={`${id}-error`} className="channel-field-error">
          {message}
        </p>
      ) : null}
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
  hint,
  validationMessage,
}: {
  path?: string;
  label: string;
  value: unknown;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
  step?: number | 'any';
  hint?: string;
  validationMessage?: string;
}) {
  const validationPath = useValidationPath(path);
  const id = useId();
  const serverError = useServerFieldError(validationPath);
  const message = validationMessage ?? serverError;
  const ref = useFieldValidity(message);
  return (
    <div className="channel-value-field" data-validation-path={validationPath} ref={ref}>
      <label htmlFor={id}>{label}</label>
      <Input
        id={id}
        type="number"
        aria-invalid={message ? true : undefined}
        aria-describedby={
          [hint ? `${id}-hint` : '', message ? `${id}-error` : ''].filter(Boolean).join(' ') ||
          undefined
        }
        value={typeof value === 'number' && Number.isFinite(value) ? value : ''}
        min={min}
        max={max}
        step={step}
        onChange={(e) => onChange(e.target.valueAsNumber)}
        required
      />
      {hint ? (
        <p id={`${id}-hint`} className="channel-field-hint">
          {hint}
        </p>
      ) : null}
      {message ? (
        <p id={`${id}-error`} className="channel-field-error">
          {message}
        </p>
      ) : null}
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
  path,
  label,
  value,
  onChange,
  disabled,
  hint,
}: {
  path?: string;
  label: string;
  value: unknown;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  hint?: string;
}) {
  const validationPath = useValidationPath(path);
  return (
    <label className="channel-toggle-field" data-validation-path={validationPath}>
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
  validationMessage,
}: {
  path?: string;
  label: string;
  value: readonly string[];
  onChange: (v: string[]) => void;
  max?: number;
  requiredItems?: boolean;
  validationMessage?: string;
}) {
  const errorId = useId();
  const serverError = useServerFieldError(useValidationPath(path));
  const error = validationMessage ?? serverError;
  return (
    <Section title={label} path={path}>
      {error ? (
        <p id={errorId} className="channel-field-error">
          {error}
        </p>
      ) : null}
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
      <Button
        aria-describedby={validationMessage ? errorId : undefined}
        disabled={value.length >= max}
        onClick={() => onChange([...value, ''])}
      >
        Add {label}
      </Button>
    </Section>
  );
}
