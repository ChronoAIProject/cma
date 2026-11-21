import { useState } from 'react';
import { Button } from '../../ui/Button';
import { Choice, NumberField, Section, TextField, Toggle } from './Fields';
import { object, without } from './model';
const kind = (v: unknown) =>
  v === null ? 'null' : Array.isArray(v) ? 'list' : typeof v === 'object' ? 'object' : typeof v;
function empty(type: string): unknown {
  const values: Record<string, unknown> = {
    string: '',
    number: 0,
    boolean: false,
    object: {},
    list: [],
    null: null,
  };
  return values[type] ?? null;
}
/** Typed arbitrary values, for schema and provider environment extensions only. */
export function ValueBuilder({
  label,
  value,
  onChange,
  depth = 0,
}: {
  label: string;
  value: unknown;
  onChange: (v: unknown) => void;
  depth?: number;
}) {
  const [newKey, setNewKey] = useState('');
  const [newType, setNewType] = useState('string');
  const type = kind(value);
  if (depth > 12) return <p>{label}: nested value retained unchanged (too deep to edit here).</p>;
  if (type === 'object' || type === 'list') {
    const entries =
      type === 'list'
        ? (value as unknown[]).map((v, i) => [String(i), v] as const)
        : Object.entries(object(value));
    const replace = (key: string, next: unknown) =>
      onChange(
        type === 'list'
          ? (value as unknown[]).map((v, i) => (String(i) === key ? next : v))
          : { ...object(value), [key]: next },
      );
    return (
      <Section title={label}>
        {entries.map(([key, item], index) => (
          <div className="channel-value-field" key={key}>
            <ValueBuilder
              label={type === 'list' ? `${label} ${index + 1}` : key}
              value={item}
              onChange={(next) => replace(key, next)}
              depth={depth + 1}
            />
            <Button
              aria-label={`Remove ${label} ${key}`}
              onClick={() =>
                onChange(
                  type === 'list'
                    ? (value as unknown[]).filter((_, i) => String(i) !== key)
                    : without(object(value), [key]),
                )
              }
            >
              Remove {type === 'list' ? 'item' : 'field'}
            </Button>
          </div>
        ))}
        <div className="channel-form-row">
          {type === 'object' ? (
            <TextField label={`${label} field name`} value={newKey} onChange={setNewKey} />
          ) : null}
          <Choice
            label={`${label} value type`}
            value={newType}
            options={['string', 'number', 'boolean', 'object', 'list', 'null']}
            onChange={setNewType}
          />
          <Button
            disabled={type === 'object' && (!newKey.trim() || Object.hasOwn(object(value), newKey))}
            onClick={() => {
              onChange(
                type === 'list'
                  ? [...(value as unknown[]), empty(newType)]
                  : { ...object(value), [newKey]: empty(newType) },
              );
              setNewKey('');
            }}
          >
            Add {type === 'list' ? 'item' : 'field'}
          </Button>
        </div>
      </Section>
    );
  }
  if (type === 'boolean') return <Toggle label={label} value={value} onChange={onChange} />;
  if (type === 'number')
    return (
      <NumberField
        label={label}
        value={value}
        onChange={onChange}
        min={-Number.MAX_VALUE}
        step="any"
      />
    );
  if (type === 'null') return <p>{label}: empty value</p>;
  return (
    <TextField
      label={label}
      value={value}
      onChange={onChange}
      multiline={typeof value === 'string' && value.includes('\n')}
    />
  );
}
