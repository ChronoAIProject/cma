import { useState } from 'react';
import { Button } from './Button';
import { Menu, type MenuEntry } from './Menu';
import { IconChevronDownOutline14 } from './icons';
import './Picker.css';

/**
 * Choosing one value from a list — `docs/Web_Style.md` §7.
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
}

export function Picker<Value extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  filter = false,
  id,
}: PickerProps<Value>) {
  const [open, setOpen] = useState(false);
  const chosen = options.find((option) => option.value === value);

  const items: MenuEntry[] = options.map((option) => ({
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
          {option.label}
          <span className="picker__reason">{option.unavailable}</span>
        </span>
      ),
    disabled: option.unavailable !== undefined,
  }));

  return (
    <Menu
      open={open}
      label={label}
      selectedId={value}
      items={items}
      /*
       * Always, rather than only where something is known to clip. In place,
       * the list is absolutely positioned inside the anchor and is cut by the
       * nearest scrolling ancestor — in a dialog that is `.modal__body`, whose
       * `overflow-y: auto` left the profile picker showing two of its five
       * options and no way to reach the rest.
       *
       * A picker cannot know what encloses it, so making this conditional
       * would be making a guess that is wrong the first time one is placed
       * somewhere new. Portalled, the list is fixed to the viewport and
       * `placeMenu` nudges it back inside, which is right in both cases.
       */
      portal
      placement={filter ? 'picker--filter' : 'picker'}
      onSelect={(id_) => {
        setOpen(false);
        onChange(id_ as Value);
      }}
      onClose={() => {
        setOpen(false);
      }}
      anchor={
        <Button
          id={id}
          variant="outline"
          placement="picker__anchor"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={label}
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
