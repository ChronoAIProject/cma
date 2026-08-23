import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  IconChevronDownOutline14,
  IconChevronUpOutline14,
  IconPlusOutline16,
  IconTrashOutline16,
} from '../ui/icons';

export const MAX_U32 = 4_294_967_295;

interface ProseBlocksProps {
  readonly label: string;
  readonly empty: string;
  readonly blocks: readonly string[];
  readonly onChange: (blocks: readonly string[]) => void;
}

export function ProseBlocks({ label, empty, blocks, onChange }: ProseBlocksProps) {
  function move(index: number, direction: -1 | 1): void {
    const destination = index + direction;
    if (destination < 0 || destination >= blocks.length) {
      return;
    }
    const reordered = [...blocks];
    const current = reordered[index];
    const neighbor = reordered[destination];
    if (current === undefined || neighbor === undefined) {
      return;
    }
    reordered[index] = neighbor;
    reordered[destination] = current;
    onChange(reordered);
  }

  return (
    <div className="field platform-settings__blocks">
      <div className="platform-settings__blocks-head">
        <span className="field__label">{label}</span>
        <Button
          size="sm"
          icon={<IconPlusOutline16 />}
          onClick={() => {
            onChange([...blocks, '']);
          }}
        >
          Add block
        </Button>
      </div>
      {blocks.length === 0 ? (
        <p className="platform-settings__empty">{empty}</p>
      ) : (
        <ol className="platform-settings__block-list">
          {blocks.map((block, index) => (
            <li className="platform-settings__block" key={index}>
              <textarea
                className="field__input platform-settings__textarea"
                rows={4}
                aria-label={`${label}, block ${String(index + 1)}`}
                value={block}
                onChange={(event) => {
                  const changed = [...blocks];
                  changed[index] = event.target.value;
                  onChange(changed);
                }}
              />
              <div className="platform-settings__block-actions">
                <Button
                  size="sm"
                  placement="platform-settings__icon"
                  icon={<IconChevronUpOutline14 />}
                  aria-label={`Move ${label.toLowerCase()} block ${String(index + 1)} up`}
                  title="Move up"
                  disabled={index === 0}
                  onClick={() => {
                    move(index, -1);
                  }}
                />
                <Button
                  size="sm"
                  placement="platform-settings__icon"
                  icon={<IconChevronDownOutline14 />}
                  aria-label={`Move ${label.toLowerCase()} block ${String(index + 1)} down`}
                  title="Move down"
                  disabled={index === blocks.length - 1}
                  onClick={() => {
                    move(index, 1);
                  }}
                />
                <Button
                  size="sm"
                  placement="platform-settings__icon"
                  icon={<IconTrashOutline16 />}
                  aria-label={`Remove ${label.toLowerCase()} block ${String(index + 1)}`}
                  title="Remove block"
                  onClick={() => {
                    onChange(blocks.filter((_, blockIndex) => blockIndex !== index));
                  }}
                />
              </div>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

interface BriefingFieldProps {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly multiline?: boolean;
  readonly onChange: (value: string) => void;
}

export function BriefingField({
  id,
  label,
  value,
  multiline = false,
  onChange,
}: BriefingFieldProps) {
  return (
    <div className="field platform-settings__field">
      <label className="field__label" htmlFor={id}>
        {label}
      </label>
      {multiline ? (
        <textarea
          id={id}
          className="field__input platform-settings__textarea"
          rows={4}
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
          }}
        />
      ) : (
        <Input
          id={id}
          placement="platform-settings__input"
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
          }}
        />
      )}
    </div>
  );
}

interface RangeFieldsProps {
  readonly id: string;
  readonly label: string;
  readonly unit: string;
  readonly minimum: number;
  readonly maximum: number;
  readonly error?: string;
  readonly onChange: (minimum: number, maximum: number) => void;
}

export function RangeFields({
  id,
  label,
  unit,
  minimum,
  maximum,
  error,
  onChange,
}: RangeFieldsProps) {
  return (
    <fieldset className="platform-settings__range">
      <legend className="platform-settings__range-title">
        {label} <span>{unit}</span>
      </legend>
      <div className="platform-settings__range-fields">
        <NumberField
          id={`${id}-minimum`}
          label={`Minimum ${label}`}
          value={minimum}
          error={error}
          onChange={(value) => {
            onChange(value, maximum);
          }}
        />
        <NumberField
          id={`${id}-maximum`}
          label={`Maximum ${label}`}
          value={maximum}
          error={error}
          onChange={(value) => {
            onChange(minimum, value);
          }}
        />
      </div>
      {error === undefined ? null : (
        <p className="field__error" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}

interface NumberFieldProps {
  readonly id: string;
  readonly label: string;
  readonly value: number;
  readonly error?: string;
  readonly minimum?: number;
  readonly maximum?: number;
  readonly onChange: (value: number) => void;
}

export function NumberField({
  id,
  label,
  value,
  error,
  minimum = 1,
  maximum = MAX_U32,
  onChange,
}: NumberFieldProps) {
  return (
    <div className="field platform-settings__number-field">
      <label className="field__label" htmlFor={id}>
        {label}
      </label>
      <Input
        id={id}
        placement="platform-settings__number"
        type="number"
        min={String(minimum)}
        max={String(maximum)}
        step="1"
        inputMode="numeric"
        aria-invalid={error === undefined ? undefined : true}
        value={Number.isFinite(value) ? String(value) : ''}
        onChange={(event) => {
          onChange(event.target.value === '' ? Number.NaN : Number(event.target.value));
        }}
      />
      {label === 'Maximum live sessions' && error !== undefined ? (
        <p className="field__error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
