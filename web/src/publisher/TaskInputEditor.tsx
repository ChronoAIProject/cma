import type { ChangeEvent } from 'react';
import type { TriggerInputFieldDefinition } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  IconChevronDownOutline14,
  IconChevronUpOutline14,
  IconPlusOutline16,
  IconTrashOutline16,
} from '../ui/icons';
import { defaultForKind, fieldKindName, type PublisherValidation } from './model';

interface TaskInputEditorProps {
  readonly fields: readonly TriggerInputFieldDefinition[];
  readonly validation: PublisherValidation;
  readonly onChange: (fields: readonly TriggerInputFieldDefinition[]) => void;
}

function number(event: ChangeEvent<HTMLInputElement>): number {
  return Number.isNaN(event.target.valueAsNumber) ? 0 : event.target.valueAsNumber;
}

function nextKey(fields: readonly TriggerInputFieldDefinition[]): string {
  let sequence = fields.length + 1;
  while (fields.some((field) => field.key === `input_${String(sequence)}`)) sequence += 1;
  return `input_${String(sequence)}`;
}

function kindFor(name: string): TriggerInputFieldDefinition['kind'] {
  if (name === 'integer') return { integer: { minimum: 0, maximum: 100 } };
  if (name === 'boolean') return 'boolean';
  if (name === 'choice') return { choice: { values: ['first', 'second'] } };
  return { text: { min_bytes: 0, max_bytes: 100 } };
}

function defaultInput(
  field: TriggerInputFieldDefinition,
  update: (change: Partial<TriggerInputFieldDefinition>) => void,
) {
  const value = field.default;
  if (field.kind === 'boolean') {
    return (
      <label className="publisher-toggle">
        <input
          type="checkbox"
          checked={value === true}
          onChange={(event) => {
            update({ default: event.target.checked });
          }}
        />
        <span>Default enabled</span>
      </label>
    );
  }
  if ('integer' in field.kind) {
    return (
      <Input
        type="number"
        value={typeof value === 'number' ? value : 0}
        min={field.kind.integer.minimum}
        max={field.kind.integer.maximum}
        onChange={(event) => {
          update({ default: number(event) });
        }}
      />
    );
  }
  if ('choice' in field.kind) {
    return (
      <select
        className="publisher-select"
        value={typeof value === 'string' ? value : (field.kind.choice.values[0] ?? '')}
        onChange={(event) => {
          update({ default: event.target.value });
        }}
      >
        {field.kind.choice.values.map((choice) => (
          <option key={choice} value={choice}>
            {choice}
          </option>
        ))}
      </select>
    );
  }
  return (
    <Input
      value={typeof value === 'string' ? value : ''}
      onChange={(event) => {
        update({ default: event.target.value });
      }}
    />
  );
}

export function TaskInputEditor({ fields, validation, onChange }: TaskInputEditorProps) {
  const replace = (index: number, field: TriggerInputFieldDefinition) => {
    onChange(fields.map((current, at) => (at === index ? field : current)));
  };

  return (
    <div className="publisher-inputs">
      {fields.map((field, index) => {
        const path = `task.input_fields.${String(index)}`;
        const kind = fieldKindName(field);
        const textBounds =
          field.kind !== 'boolean' && 'text' in field.kind ? field.kind.text : null;
        const integerBounds =
          field.kind !== 'boolean' && 'integer' in field.kind ? field.kind.integer : null;
        const choiceValues =
          field.kind !== 'boolean' && 'choice' in field.kind ? field.kind.choice.values : null;
        const hasDefault = field.required || field.default !== undefined;
        const update = (change: Partial<TriggerInputFieldDefinition>) => {
          replace(index, { ...field, ...change });
        };
        return (
          <fieldset className="publisher-input" key={`${String(index)}:${field.key}`}>
            <legend>Input {String(index + 1)}</legend>
            <div className="publisher-input__commands">
              <Button
                size="sm"
                aria-label={`Move ${field.key || `input ${String(index + 1)}`} up`}
                title="Move up"
                icon={<IconChevronUpOutline14 />}
                disabled={index === 0}
                onClick={() => {
                  const next = [...fields];
                  const previous = next[index - 1];
                  if (previous === undefined) return;
                  next[index - 1] = field;
                  next[index] = previous;
                  onChange(next);
                }}
              />
              <Button
                size="sm"
                aria-label={`Move ${field.key || `input ${String(index + 1)}`} down`}
                title="Move down"
                icon={<IconChevronDownOutline14 />}
                disabled={index === fields.length - 1}
                onClick={() => {
                  const next = [...fields];
                  const following = next[index + 1];
                  if (following === undefined) return;
                  next[index] = following;
                  next[index + 1] = field;
                  onChange(next);
                }}
              />
              <Button
                size="sm"
                variant="danger"
                aria-label={`Remove ${field.key || `input ${String(index + 1)}`}`}
                title="Remove input"
                icon={<IconTrashOutline16 />}
                onClick={() => {
                  onChange(fields.filter((_, at) => at !== index));
                }}
              />
            </div>

            <div className="publisher-fields publisher-fields--three">
              <label className="publisher-field">
                <span>Key</span>
                <Input
                  value={field.key}
                  aria-invalid={validation[`${path}.key`] === undefined ? undefined : true}
                  onChange={(event) => {
                    update({ key: event.target.value });
                  }}
                />
                {validation[`${path}.key`] === undefined ? null : (
                  <small role="alert">{validation[`${path}.key`]}</small>
                )}
              </label>
              <label className="publisher-field">
                <span>Kind</span>
                <select
                  className="publisher-select"
                  value={kind}
                  onChange={(event) => {
                    const next = event.target.value;
                    update({ kind: kindFor(next), default: defaultForKind(next) });
                  }}
                >
                  <option value="text">Text</option>
                  <option value="integer">Integer</option>
                  <option value="boolean">Boolean</option>
                  <option value="choice">Choice</option>
                </select>
              </label>
              <div className="publisher-field publisher-field--checks">
                <span>Behavior</span>
                <label className="publisher-toggle">
                  <input
                    type="checkbox"
                    checked={field.required}
                    onChange={(event) => {
                      update({
                        required: event.target.checked,
                        ...(event.target.checked && field.default === undefined
                          ? { default: defaultForKind(kind) }
                          : {}),
                      });
                    }}
                  />
                  <span>Required</span>
                </label>
                <label className="publisher-toggle">
                  <input
                    type="checkbox"
                    checked={field.editable}
                    onChange={(event) => {
                      update({ editable: event.target.checked });
                    }}
                  />
                  <span>Contributor may edit</span>
                </label>
              </div>
            </div>

            {field.kind === 'boolean' ? null : textBounds !== null ? (
              <div className="publisher-fields">
                <label className="publisher-field">
                  <span>Minimum bytes</span>
                  <Input
                    type="number"
                    min={0}
                    max={8192}
                    value={textBounds.min_bytes}
                    onChange={(event) => {
                      update({ kind: { text: { ...textBounds, min_bytes: number(event) } } });
                    }}
                  />
                </label>
                <label className="publisher-field">
                  <span>Maximum bytes</span>
                  <Input
                    type="number"
                    min={0}
                    max={8192}
                    value={textBounds.max_bytes}
                    onChange={(event) => {
                      update({ kind: { text: { ...textBounds, max_bytes: number(event) } } });
                    }}
                  />
                </label>
              </div>
            ) : integerBounds !== null ? (
              <div className="publisher-fields">
                <label className="publisher-field">
                  <span>Minimum</span>
                  <Input
                    type="number"
                    value={integerBounds.minimum}
                    onChange={(event) => {
                      update({
                        kind: { integer: { ...integerBounds, minimum: number(event) } },
                      });
                    }}
                  />
                </label>
                <label className="publisher-field">
                  <span>Maximum</span>
                  <Input
                    type="number"
                    value={integerBounds.maximum}
                    onChange={(event) => {
                      update({
                        kind: { integer: { ...integerBounds, maximum: number(event) } },
                      });
                    }}
                  />
                </label>
              </div>
            ) : (
              <label className="publisher-field">
                <span>Choices, one per line</span>
                <textarea
                  className="field__input publisher-textarea publisher-textarea--short"
                  value={(choiceValues ?? []).join('\n')}
                  onChange={(event) => {
                    update({ kind: { choice: { values: event.target.value.split('\n') } } });
                  }}
                />
              </label>
            )}
            {validation[`${path}.bounds`] === undefined ? null : (
              <p className="publisher-field-error" role="alert">
                {validation[`${path}.bounds`]}
              </p>
            )}
            {validation[`${path}.choices`] === undefined ? null : (
              <p className="publisher-field-error" role="alert">
                {validation[`${path}.choices`]}
              </p>
            )}

            <div className="publisher-default">
              {field.required ? null : (
                <label className="publisher-toggle">
                  <input
                    type="checkbox"
                    checked={hasDefault}
                    onChange={(event) => {
                      replace(index, {
                        ...field,
                        ...(event.target.checked
                          ? { default: defaultForKind(kind) }
                          : { default: undefined }),
                      });
                    }}
                  />
                  <span>Provide a default</span>
                </label>
              )}
              {hasDefault ? (
                <label className="publisher-field">
                  <span>Default</span>
                  {defaultInput(field, update)}
                  {validation[`${path}.default`] === undefined ? null : (
                    <small role="alert">{validation[`${path}.default`]}</small>
                  )}
                </label>
              ) : null}
            </div>
          </fieldset>
        );
      })}

      <Button
        size="sm"
        icon={<IconPlusOutline16 />}
        disabled={fields.length >= 32}
        onClick={() => {
          onChange([
            ...fields,
            {
              key: nextKey(fields),
              kind: { text: { min_bytes: 0, max_bytes: 100 } },
              required: false,
              editable: true,
            },
          ]);
        }}
      >
        Add task input
      </Button>
    </div>
  );
}
