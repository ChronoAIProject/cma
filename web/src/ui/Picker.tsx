import { useState } from 'react';
import { Button, type ButtonSize } from './Button';
import { Menu, type MenuEntry } from './Menu';
import { IconChevronDownOutline14 } from './icons';
import './Picker.css';

/**
 * Choosing one value from a list — the shared style contracts
 *
 * A button and a menu, packaged. Every picker needs the same open state, and
 * seven call sites each holding their own copy is seven places for it to be
 * held slightly differently — which is what this replaced.
 *
 * An option that cannot be chosen is **disabled and still listed**, with the
 * reason on the row. Removing it answers "why is it not there" with nothing,
 * and a person who came looking for it concludes the product cannot do it.
 */

export interface PickerOption<Value extends string> {
  readonly value: Value;
  readonly label: string;
  /** Why this cannot be chosen. Disabled, never removed. */
  readonly unavailable?: string;
}

interface PickerProps<Value extends string> {
  readonly searchable?: boolean;
  /** Names the control for a screen reader, and never drawn by it. */
  readonly label: string;
  readonly value: Value;
  readonly options: readonly PickerOption<Value>[];
  readonly onChange: (value: Value) => void;
  readonly disabled?: boolean;
  /** Holds a minimum width so a filter's row does not resize as it changes. */
  readonly filter?: boolean;
  /** Associates the control with a label rendered outside it. */
  readonly id?: string;
  readonly describedBy?: string;
  readonly invalid?: boolean;
  /**
   * A class for placing this picker in its row.
   *
   * Offered so a caller can size it without writing a rule for `.picker`, the
   * primitive's own root — which is how an unrelated selection list came to
   * hand every picker in the product an `overflow-y` it never asked for.
   */
  readonly placement?: string;
  readonly size?: ButtonSize;
}

export function Picker<Value extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  filter = false,
  id,
  describedBy,
  invalid,
  placement,
  size,
  searchable,
}: PickerProps<Value>) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const chosen = options.find((option) => option.value === value);

  const query = search.trim().toLocaleLowerCase();
  const items: MenuEntry[] = options
    .filter((option) => !searchable || option.label.toLocaleLowerCase().includes(query))
    .map((option) => ({
      id: option.value,
      /*
       * The reason rides with the row rather than in a tooltip: §7 wants a
       * disabled control to say why where it is, and a tooltip is not there for
       * anybody reading with the keyboard.
       */
      label:
        option.unavailable === undefined ? (
          option.label
        ) : (
          <span className="picker__unavailable">
            {option.label} <span className="picker__reason">{option.unavailable}</span>
          </span>
        ),
      disabled: option.unavailable !== undefined,
    }));

  return (
    <Menu
      open={open}
      label={label}
      search={
        searchable
          ? { label: `Search ${label.toLowerCase()}`, value: search, onChange: setSearch }
          : undefined
      }
      footer={
        searchable && items.length === 0 ? <p role="status">No matching choices.</p> : undefined
      }
      selectedId={value}
      items={items}
      placement={[filter ? 'picker--filter' : 'picker', placement].filter(Boolean).join(' ')}
      onSelect={(id_) => {
        setOpen(false);
        setSearch('');
        onChange(id_ as Value);
      }}
      onClose={() => {
        setOpen(false);
        setSearch('');
      }}
      anchor={
        <Button
          size={size}
          title={`${label}: ${chosen?.label ?? value}`}
          id={id}
          variant="outline"
          placement="picker__anchor"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={label}
          aria-describedby={describedBy}
          aria-invalid={invalid || undefined}
          disabled={disabled}
          onClick={() => {
            setOpen((was) => !was);
          }}
        >
          <span className="picker__value">{chosen?.label ?? ''}</span>
          <IconChevronDownOutline14 className="picker__caret" />
        </Button>
      }
    />
  );
}
