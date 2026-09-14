import { useId } from 'react';
import { Button } from '../../ui/Button';
import { IconPlusOutline16 } from '../../ui/icons';
import { fieldDescription } from '../Field';
import type { FieldErrors } from '../validation';
import { moveItem, OrderedActions } from './OrderedActions';

export function CommandList({
  value,
  onChange,
  label,
  help,
  maximum,
  disabled,
  errors,
  field,
}: {
  readonly value: readonly string[];
  readonly onChange: (value: readonly string[]) => void;
  readonly label: string;
  readonly help: string;
  readonly maximum: number;
  readonly disabled: boolean;
  readonly errors: FieldErrors;
  readonly field: string;
}) {
  const id = useId();
  const error = errors[field] ?? errors.commands;
  return (
    <div className="profile-field">
      <p id={`${id}-help`} className="profile-field__help">
        {help}
      </p>
      <ol className="profile-ordered-list">
        {value.map((command, index) => {
          const key = `${field}.${String(index)}`;
          const inputId = `profile-${key}`;
          return (
            <li key={index}>
              <label className="sr-only" htmlFor={inputId}>
                {label} {index + 1}
              </label>
              <textarea
                id={inputId}
                className="profile-textarea profile-command"
                rows={2}
                value={command}
                disabled={disabled}
                aria-invalid={errors[key] === undefined ? undefined : true}
                aria-describedby={
                  errors[key] === undefined ? `${id}-help` : `${id}-help ${inputId}-error`
                }
                onChange={(event) =>
                  onChange(
                    value.map((current, at) => (at === index ? event.target.value : current)),
                  )
                }
              />
              <OrderedActions
                label={`${label.toLowerCase()} ${String(index + 1)}`}
                index={index}
                length={value.length}
                disabled={disabled}
                onMove={(to) => onChange(moveItem(value, index, to))}
                onRemove={() => onChange(value.filter((_, at) => at !== index))}
              />
              {errors[key] === undefined ? null : (
                <p id={`${inputId}-error`} className="profile-field__error">
                  {errors[key]}
                </p>
              )}
            </li>
          );
        })}
      </ol>
      {error === undefined ? null : (
        <p id={`profile-${field}-error`} className="profile-field__error">
          {error}
        </p>
      )}
      <Button
        type="button"
        icon={<IconPlusOutline16 />}
        disabled={disabled || value.length >= maximum}
        aria-describedby={fieldDescription(id)}
        onClick={() => onChange([...value, ''])}
      >
        Add command
      </Button>
    </div>
  );
}
