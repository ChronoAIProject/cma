import { useEffect, useLayoutEffect, useRef } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { placeMenu, type MenuAlign, type MenuSide } from './menuPlacement';
import { presenceAttributes, usePresence, usePresenceExiting } from './Presence';
import './Popover.css';

/**
 * A small card of controls opened from a trigger — `docs/Web_Style.md` §13.
 *
 * The dropdown (§12.3) holds rows; this holds anything else, such as a
 * slider beside a value list. It follows the dropdown's rules for the same
 * reasons: open state belongs to the owner, and the card is drawn on the body
 * from the trigger's box so a scrolling or clipping ancestor cannot cut it off.
 *
 * A dropdown opened from inside the card is part of it. Its list is portalled
 * too, outside the card's DOM, so "inside" is decided by the React tree — a
 * press or focus there bubbles through the card's handlers even though the
 * list is elsewhere in the document.
 */

/** Laid out but invisible until placed, as the dropdown's list is. */
const MEASURING: CSSProperties = { visibility: 'hidden', left: 0, top: 0 };

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), [tabindex]:not([tabindex="-1"])';

export function Popover({
  open,
  anchor,
  label,
  onClose,
  children,
  side = 'top',
  align = 'end',
  placement,
  cardClassName,
  autoFocus = true,
}: {
  readonly open: boolean;
  /** The trigger, rendered in place. */
  readonly anchor: ReactNode;
  /** The card's accessible name. */
  readonly label: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  readonly side?: MenuSide;
  readonly align?: MenuAlign;
  /** Extra class on the in-place root, for placing the trigger in its row. */
  readonly placement?: string;
  /** Extra class on the card, for its owner's contents. */
  readonly cardClassName?: string;
  /** Hover previews leave keyboard focus where it is. */
  readonly autoFocus?: boolean;
}) {
  const rootRef = useRef<HTMLSpanElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const pressedInside = useRef<Event | undefined>(undefined);
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const parentExiting = usePresenceExiting();
  const interactive = open && !parentExiting;
  const { present, exiting } = usePresence(interactive);

  useLayoutEffect(() => {
    if (!interactive) return;
    const place = () => {
      const rect = rootRef.current?.getBoundingClientRect();
      const card = cardRef.current;
      if (rect === undefined || card === null) return;
      const at = placeMenu(
        rect,
        { width: card.offsetWidth, height: card.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
        side,
        align,
      );
      card.style.left = `${String(at.left)}px`;
      card.style.top = `${String(at.top)}px`;
      card.style.visibility = 'visible';
    };
    place();
    window.addEventListener('scroll', place, true);
    window.addEventListener('resize', place);
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
    if (cardRef.current) observer?.observe(cardRef.current);
    if (rootRef.current) observer?.observe(rootRef.current);
    return () => {
      observer?.disconnect();
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('resize', place);
    };
  }, [interactive, side, align]);

  useEffect(() => {
    if (!interactive) return;
    const onPointerDown = (event: PointerEvent) => {
      // Recorded by the card's own handler, which a nested portalled list reaches too.
      if (pressedInside.current === event) return;
      if (event.target instanceof Node && rootRef.current?.contains(event.target) === true) return;
      onClose();
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [interactive, onClose]);

  useEffect(() => () => clearTimeout(blurTimer.current), []);

  // Focus moves into the card when it opens, so its controls are reachable at once.
  useEffect(() => {
    if (!interactive || !autoFocus) return;
    const card = cardRef.current;
    const first =
      card?.querySelector<HTMLElement>('[data-autofocus]') ??
      card?.querySelector<HTMLElement>(FOCUSABLE);
    first?.focus({ preventScroll: true });
  }, [interactive, autoFocus]);

  // And back to the trigger when it closes, unless somebody pressed elsewhere.
  const wasOpen = useRef(false);
  useEffect(() => {
    const closing = wasOpen.current && !interactive;
    wasOpen.current = interactive;
    if (!closing || parentExiting || !autoFocus) return;
    const active = document.activeElement;
    if (active === null || active === document.body || cardRef.current?.contains(active) === true)
      rootRef.current?.querySelector<HTMLElement>('button')?.focus();
  }, [interactive, parentExiting, autoFocus]);

  const card = !present ? null : (
    <div
      ref={cardRef}
      role="dialog"
      aria-label={label}
      className={['popover', cardClassName ?? ''].filter(Boolean).join(' ')}
      style={MEASURING}
      {...presenceAttributes(exiting)}
      onPointerDown={(event) => {
        pressedInside.current = event.nativeEvent;
      }}
      onFocus={() => clearTimeout(blurTimer.current)}
      onBlur={(event) => {
        const next = event.relatedTarget;
        // A press on nothing focusable is the outside-press handler's to judge.
        if (next === null) return;
        if (rootRef.current?.contains(next) === true || cardRef.current?.contains(next) === true)
          return;
        // Focus leaving for a nested portalled list comes straight back through
        // onFocus above and cancels this; leaving for anywhere else closes.
        blurTimer.current = setTimeout(onClose, 0);
      }}
      // A portal's events still bubble through the React tree; keep a press in
      // the card from reaching the trigger's own handlers.
      onClick={(event) => event.stopPropagation()}
    >
      {children}
    </div>
  );

  return (
    <span
      ref={rootRef}
      className={['popover-root', placement ?? ''].filter(Boolean).join(' ')}
      onKeyDown={(event) => {
        // A nested dropdown stops its own Escape, so this closes only the card.
        if (interactive && event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      {anchor}
      {card === null ? null : createPortal(card, document.body)}
    </span>
  );
}
