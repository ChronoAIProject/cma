import { useEffect, useId, useRef, useState } from 'react';
import './Select.css';

/**
 * The one dropdown — docs/Web_Style.md §7.
 *
 * An instrument closed and a plate open: hairline and fill at rest with no
 * impression, since it is not a key; the open menu floats and takes the card
 * impression.
 *
 * Choosing a VALUE is a select and choosing an ACTION is a menu. They look
 * alike and behave differently, so this one never runs anything when it opens
 * — it reports a choice, and the caller decides what that means.
 *
 * The caret flips rather than rotating. At 10px a rotation reads as a wobble,
 * so the glyph is swapped in CSS content and there is nothing to animate.
 *
 * Replacing a native `<select>` costs the keyboard behaviour the platform gave
 * away for free, so it is implemented rather than lost: the trigger keeps
 * focus and points at the active option through `aria-activedescendant`,
 * arrows and Home/End move it, Enter and Space choose, Escape closes without
 * choosing. §10 requires every view to be fully keyboard operable, and a
 * dropdown that can only be clicked is not.
 */

export interface SelectOption<Value extends string> {
  readonly value: Value;
  readonly label: string;
  /** Why this option cannot be chosen. Disabled, never removed (§7). */
  readonly unavailable?: string;
}

interface SelectProps<Value extends string> {
  readonly label: string;
  readonly value: Value;
  readonly options: readonly SelectOption<Value>[];
  readonly onChange: (value: Value) => void;
  readonly disabled?: boolean;
  /**
   * Holds the 160px minimum §7 gives a filter, so its row does not resize as
   * the value changes.
   */
  readonly filter?: boolean;
  /** Associates the instrument with a label rendered outside it. */
  readonly id?: string;
}

/** Step to the next option that can actually be chosen. */
function step<Value extends string>(
  options: readonly SelectOption<Value>[],
  from: number,
  delta: number,
): number {
  for (let i = from + delta; i >= 0 && i < options.length; i += delta) {
    if (options[i]?.unavailable === undefined) {
      return i;
    }
  }
  return from;
}

function firstAvailable<Value extends string>(
  options: readonly SelectOption<Value>[],
  delta: number,
): number {
  const start = delta > 0 ? -1 : options.length;
  return step(options, start, delta);
}

export function Select<Value extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  filter = false,
  id,
}: SelectProps<Value>) {
  const [open, setOpen] = useState(false);
  const selectedIndex = options.findIndex((option) => option.value === value);
  const [activeIndex, setActiveIndex] = useState(selectedIndex);
  const rootRef = useRef<HTMLDivElement>(null);
  const generatedId = useId();
  const listId = `${generatedId}-list`;
  const optionId = (index: number) => `${generatedId}-option-${String(index)}`;

  // A plate that can only be dismissed by choosing something traps whoever
  // opened it by mistake.
  useEffect(() => {
    if (!open) {
      return;
    }
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
    };
  }, [open]);

  const choose = (index: number) => {
    const option = options[index];
    if (option === undefined || option.unavailable !== undefined) {
      return;
    }
    setOpen(false);
    onChange(option.value);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    switch (event.key) {
      case 'Escape':
        if (open) {
          event.preventDefault();
          setOpen(false);
        }
        return;
      case 'ArrowDown':
      case 'ArrowUp': {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        if (!open) {
          setOpen(true);
          setActiveIndex(selectedIndex >= 0 ? selectedIndex : firstAvailable(options, delta));
          return;
        }
        setActiveIndex((current) => step(options, current, delta));
        return;
      }
      case 'Home':
      case 'End':
        if (open) {
          event.preventDefault();
          setActiveIndex(firstAvailable(options, event.key === 'Home' ? 1 : -1));
        }
        return;
      case 'Enter':
      case ' ':
        event.preventDefault();
        if (open) {
          choose(activeIndex);
        } else {
          setOpen(true);
          setActiveIndex(selectedIndex >= 0 ? selectedIndex : firstAvailable(options, 1));
        }
        return;
      default:
        return;
    }
  };

  const selected = options[selectedIndex];

  return (
    <div className="select-root" ref={rootRef}>
      <button
        type="button"
        id={id}
        className={`select${filter ? ' select--filter' : ''}`}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listId}
        aria-label={label}
        aria-activedescendant={open && activeIndex >= 0 ? optionId(activeIndex) : undefined}
        disabled={disabled}
        onKeyDown={onKeyDown}
        onClick={() => {
          setOpen((wasOpen) => !wasOpen);
          setActiveIndex(selectedIndex >= 0 ? selectedIndex : firstAvailable(options, 1));
        }}
      >
        <span className="select__value">{selected?.label ?? value}</span>
        <span className="select__caret" aria-hidden="true" />
      </button>

      {open ? (
        <ul className="menu select__menu" role="listbox" id={listId} aria-label={label}>
          {options.map((option, index) => (
            <li
              key={option.value}
              id={optionId(index)}
              role="option"
              className={`menu__item${index === activeIndex ? ' menu__item--active' : ''}`}
              aria-selected={option.value === value}
              aria-disabled={option.unavailable !== undefined}
              onMouseDown={(event) => {
                // Before the button's blur, so choosing with the pointer does
                // not race the outside-gesture handler that closes the plate.
                event.preventDefault();
                choose(index);
              }}
            >
              <span>{option.label}</span>
              {/*
               * Marked on the leading edge AND by a tick: §7 asks for both,
               * because a mark carried by colour alone is not a mark to
               * everyone (§10).
               */}
              {option.value === value ? <span className="menu__tick" aria-hidden="true" /> : null}
              {option.unavailable === undefined ? null : (
                <span className="select__unavailable">{option.unavailable}</span>
              )}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
