import { useEffect, useLayoutEffect, useRef } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { IconCheckOutline16 } from './icons';
import { placeMenu, type MenuAlign, type MenuSide } from './menuPlacement';
import { presenceAttributes, usePresence, usePresenceExiting } from './Presence';
import './Menu.css';

/**
 * The product's one dropdown — `docs/Web_Style.md` §12.3.
 *
 * A value picker and an overflow menu are the same control here. They were
 * two before, and the second one had no keyboard route at all.
 *
 * **Open state belongs to the owner.** A menu that closed itself could not be
 * told to stay open while something it triggered was still resolving, and
 * every caller that needed that grew its own copy of this.
 *
 * Selection is a trailing check rather than a fill, so a selected row and a
 * hovered row are different things rather than the same colour twice.
 *
 * **The list always goes on the body, positioned from the anchor's box.** It
 * used to be a choice, defaulting to rendering in place, and every caller that
 * took the default was placed wrongly: a picker inside a dialog was cut off by
 * the dialog's scrollport, and a resource list's own options menu opened 53px
 * to the left of the window. A trigger cannot know what encloses it or how
 * near an edge it sits, so the in-place default asked each caller a question
 * none of them could answer. On the body, `placeMenu` reads the anchor and the
 * viewport and nudges the list back inside, which is right in every case the
 * in-place path was right in and in the cases it was not.
 */

/** A row somebody can choose. */
export interface MenuItem {
  readonly id: string;
  readonly label: ReactNode;
  readonly disabled?: boolean;
  readonly reason?: string;
  /** A leading 16px glyph. */
  readonly icon?: ReactNode;
  /** Destructive: error-coloured, with its own hover fill. */
  readonly danger?: boolean;
  /**
   * Where choosing this row goes, when it is navigation.
   *
   * The row becomes a real link, so a modified click or a middle click opens
   * the destination in a new tab as any other link does; an ordinary click
   * still reports the selection and leaves navigation to the owner.
   */
  readonly href?: string;
}

/** A hairline between groups. Not selectable, not focusable. */
export interface MenuSeparator {
  readonly type: 'separator';
  readonly id: string;
}

/** A heading over a group. Not selectable, not focusable. */
export interface MenuLabel {
  readonly type: 'label';
  readonly id: string;
  readonly text: string;
}

/** Compact alternatives within one menu, with independent radio semantics. */
export interface MenuChoices {
  readonly type: 'choices';
  readonly id: string;
  readonly text: string;
  readonly selectedId: string;
  readonly choices: readonly { readonly id: string; readonly label: string }[];
}

export type MenuEntry = MenuItem | MenuSeparator | MenuLabel | MenuChoices;

function isChoosable(entry: MenuEntry): entry is MenuItem {
  return !('type' in entry);
}

/**
 * Laid out but invisible, so the placement effect can measure a real box.
 *
 * Without this the first frame is placed from a zero-size guess and then
 * jumps once it measures — which is most visible on the `end` alignment,
 * where the whole width is the offset.
 */
const MEASURING: CSSProperties = { visibility: 'hidden', left: 0, top: 0 };

interface MenuProps {
  readonly open: boolean;
  /** The trigger, rendered in place. */
  readonly anchor: ReactNode;
  readonly items: readonly MenuEntry[];
  readonly selectedId?: string;
  /**
   * Selected rows, where the menu holds several independent groups.
   *
   * A view-options menu is three questions on one card — what to show, in
   * what order, arranged how — and each has its own current answer. One
   * `selectedId` could only mark one of them.
   */
  readonly selectedIds?: readonly string[];
  readonly onSelect: (id: string) => void;
  readonly onClose: () => void;
  readonly align?: MenuAlign;
  readonly side?: MenuSide;
  /** Tighter rows, for a menu that is mostly a list of values. */
  readonly dense?: boolean;
  /** Extra class for placement only. */
  readonly placement?: string;
  /**
   * Hold the list to at least the anchor's width.
   *
   * For a trigger that is a row rather than a glyph, where a narrower list
   * reads as belonging to something else and a wider one overhangs it. The
   * list is on the body and fixed, so it cannot take this from the anchor in
   * CSS — the width has to be measured with the position.
   */
  readonly matchAnchorWidth?: boolean;
  /** The list's accessible name. */
  readonly label?: string;
}

export function Menu({
  open,
  anchor,
  items,
  selectedId,
  selectedIds,
  onSelect,
  onClose,
  align = 'start',
  side = 'bottom',
  dense = false,
  placement,
  matchAnchorWidth = false,
  label,
}: MenuProps) {
  const rootRef = useRef<HTMLSpanElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const parentExiting = usePresenceExiting();
  const interactive = open && !parentExiting;
  const { present, exiting } = usePresence(interactive);

  /*
   * The list is positioned by writing to its node rather than by holding
   * coordinates in state.
   *
   * Placement is a measurement of the DOM fed straight back into it, so state
   * would only be a detour: an extra render per scroll frame, and a value
   * that is stale the moment anything moves. Writing it here is the same
   * thing a layout effect exists for.
   */
  useLayoutEffect(() => {
    if (!interactive) {
      return;
    }
    const place = () => {
      const rect = rootRef.current?.getBoundingClientRect();
      const list = listRef.current;
      if (rect === undefined || list === null) {
        return;
      }
      const at = placeMenu(
        rect,
        { width: list.offsetWidth, height: list.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
        side,
        align,
      );
      list.style.left = `${String(at.left)}px`;
      list.style.top = `${String(at.top)}px`;
      if (matchAnchorWidth) {
        list.style.minWidth = `${String(Math.round(rect.width))}px`;
      }
      // Placed, so it can be seen. Until now it was laid out but hidden, so
      // the measurement above had a real box to read.
      list.style.visibility = 'visible';
    };
    place();
    /*
     * Capture phase, so a menu anchored inside a scrolling pane follows its
     * trigger rather than staying where the trigger used to be — a scroll
     * inside a nested element never reaches the window in the bubble phase.
     */
    window.addEventListener('scroll', place, true);
    window.addEventListener('resize', place);
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
    if (listRef.current) observer?.observe(listRef.current);
    if (rootRef.current) observer?.observe(rootRef.current);
    return () => {
      observer?.disconnect();
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('resize', place);
    };
  }, [interactive, side, align, matchAnchorWidth]);

  useEffect(() => {
    if (!interactive) {
      return;
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) {
        return;
      }
      // A portalled list is outside the anchor's subtree, so both are checked.
      if (rootRef.current?.contains(event.target) === true) {
        return;
      }
      if (listRef.current?.contains(event.target) === true) {
        return;
      }
      onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [interactive, onClose]);

  /*
   * Focus lands on the first row when the list opens, and on the selected one
   * where there is a selection — which is what makes a value picker usable
   * from the keyboard rather than merely reachable.
   */
  useEffect(() => {
    if (!interactive) {
      return;
    }
    const rows = rowsIn(listRef.current);
    const chosen = rows.find((row) => row.dataset.id === selectedId);
    (chosen ?? rows[0])?.focus();
  }, [interactive, selectedId]);

  /*
   * And returns to the trigger when the list closes, if it was still inside
   * it. Without this, closing leaves focus on a row that has been removed from
   * the document and the next Tab starts again at the top of the page —
   * furthest from where the person was working.
   *
   * Only when focus was in the list: a menu closed because somebody clicked
   * elsewhere must not pull them back.
   */
  const wasOpen = useRef(false);
  useEffect(() => {
    const closing = wasOpen.current && !interactive;
    wasOpen.current = interactive;
    if (!closing || parentExiting) {
      return;
    }
    const active = document.activeElement;
    const inList = active !== null && listRef.current?.contains(active) === true;
    if (inList || active === document.body) {
      rootRef.current?.querySelector('button')?.focus();
    }
  }, [interactive, parentExiting]);

  const onListKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (!interactive) return;
    const rows = rowsIn(listRef.current);
    if (rows.length === 0) {
      return;
    }
    const here = rows.indexOf(document.activeElement as HTMLElement);
    const move = (to: number) => {
      event.preventDefault();
      rows[(to + rows.length) % rows.length]?.focus();
    };

    switch (event.key) {
      case 'ArrowLeft':
      case 'ArrowRight': {
        const group = (document.activeElement as HTMLElement | null)?.closest('.menu__choices');
        if (!group) return;
        const choices = [...group.querySelectorAll<HTMLButtonElement>('button')];
        const current = choices.indexOf(document.activeElement as HTMLButtonElement);
        event.preventDefault();
        choices[
          (current + (event.key === 'ArrowRight' ? 1 : -1) + choices.length) % choices.length
        ]?.focus();
        return;
      }
      case 'ArrowDown':
        move(here + 1);
        return;
      case 'ArrowUp':
        move(here - 1);
        return;
      case 'Home':
        move(0);
        return;
      case 'End':
        move(rows.length - 1);
        return;
      default:
    }
  };

  const list = !present ? null : (
    <div
      ref={listRef}
      className={['menu', dense ? 'menu--dense' : '', 'menu--portal'].filter(Boolean).join(' ')}
      style={MEASURING}
      role="menu"
      aria-label={label}
      {...presenceAttributes(exiting)}
      onKeyDown={onListKeyDown}
      /*
       * A portal's events still bubble through the React tree, so without
       * this a row's click re-fires the trigger's own handler and reopens the
       * list it just closed.
       */
      onClick={(event) => {
        event.stopPropagation();
      }}
    >
      {items.map((entry) => (
        <MenuRow
          key={entry.id}
          entry={entry}
          selected={entry.id === selectedId || selectedIds?.includes(entry.id) === true}
          choosing={selectedId !== undefined || selectedIds !== undefined}
          onSelect={(id) => {
            if (interactive) onSelect(id);
          }}
        />
      ))}
    </div>
  );

  return (
    <span
      ref={rootRef}
      className={['menu-root', placement ?? ''].filter(Boolean).join(' ')}
      onKeyDown={(event) => {
        // Portalled rows bubble here too. Dismiss this menu before Escape
        // reaches an enclosing dialog, including when its trigger has focus.
        if (interactive && event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      {anchor}
      {list === null ? null : createPortal(list, document.body)}
    </span>
  );
}

/** Every focusable row, in order. Separators and headings are neither. */
function rowsIn(list: HTMLElement | null): HTMLElement[] {
  return list === null ? [] : [...list.querySelectorAll<HTMLElement>('.menu__item')];
}

function MenuRow({
  entry,
  selected,
  choosing,
  onSelect,
}: {
  readonly entry: MenuEntry;
  readonly selected: boolean;
  /** Whether this menu is picking a value rather than running an action. */
  readonly choosing: boolean;
  readonly onSelect: (id: string) => void;
}) {
  if ('type' in entry && entry.type === 'separator') {
    return <div className="menu__separator" role="separator" />;
  }
  if ('type' in entry && entry.type === 'label') {
    return (
      <div className="menu__label" role="presentation">
        {entry.text}
      </div>
    );
  }
  if ('type' in entry && entry.type === 'choices') {
    return (
      <div className="menu__choice-group" role="group" aria-label={entry.text}>
        <div className="menu__label" aria-hidden="true">
          {entry.text}
        </div>
        <div className="menu__choices">
          {entry.choices.map((choice) => (
            <button
              key={choice.id}
              type="button"
              role="menuitemradio"
              className="menu__item menu__choice"
              data-id={choice.id}
              aria-checked={entry.selectedId === choice.id}
              onClick={() => onSelect(choice.id)}
            >
              {choice.label}
            </button>
          ))}
        </div>
      </div>
    );
  }
  if (!isChoosable(entry)) {
    return null;
  }

  const unavailable = entry.disabled === true;
  const content = (
    <>
      {entry.icon === undefined ? null : (
        <span className="menu__icon" aria-hidden="true">
          {entry.icon}
        </span>
      )}
      <span className="menu__itemlabel">
        {entry.reason === undefined ? (
          entry.label
        ) : (
          <>
            <span className="menu__labeltext">{entry.label}</span>{' '}
            <span className="menu__reason">{entry.reason}</span>
          </>
        )}
      </span>
      {selected ? <IconCheckOutline16 className="menu__check" /> : null}
    </>
  );
  const shared = {
    /*
     * A menu with a current value is a set of alternatives, not a list of
     * actions, and `menuitemradio` is what says so — it is also the role
     * `aria-checked` is actually defined for.
     */
    role: choosing ? 'menuitemradio' : 'menuitem',
    'data-id': entry.id,
    'data-has-reason': entry.reason === undefined ? undefined : '',
    className: ['menu__item', entry.danger === true ? 'menu__item--danger' : '']
      .filter(Boolean)
      .join(' '),
    /*
     * `aria-disabled` rather than `disabled`, so the row keeps its place in
     * the keyboard order. A row that cannot be reached cannot have its
     * reason read, and the reason is the whole point of listing something
     * that cannot be chosen (§12.3).
     */
    'aria-disabled': unavailable,
    'aria-checked': choosing ? selected : undefined,
  } as const;
  if (entry.href !== undefined && !unavailable) {
    return (
      <a
        {...shared}
        href={entry.href}
        onClick={(event) => {
          // A modified click is the browser's: a new tab or window, not a
          // selection in this one.
          if (
            event.button !== 0 ||
            event.metaKey ||
            event.ctrlKey ||
            event.shiftKey ||
            event.altKey
          ) {
            return;
          }
          event.preventDefault();
          onSelect(entry.id);
        }}
      >
        {content}
      </a>
    );
  }
  return (
    <button
      type="button"
      {...shared}
      onClick={() => {
        if (unavailable) {
          return;
        }
        onSelect(entry.id);
      }}
    >
      {content}
    </button>
  );
}
