import { useEffect, useRef } from 'react';
import type { ReactNode, RefObject } from 'react';

/**
 * What every floating plate shares — docs/Web_Style.md §7.
 *
 * The focus trap, Escape, and the return of focus on close are written once,
 * here, because they are the parts nobody notices are missing until a
 * keyboard user is stranded behind a plate they cannot leave. The dialog and
 * the command bar both call this; neither carries its own copy.
 */

/** Everything inside `root` that a Tab can reach. */
export function focusable(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((element) => element.closest('[hidden], [inert]') === null);
}

/**
 * The scrim behind a plate. Click outside the plate to dismiss — written once
 * here; the keyboard route is Escape, which usePlateFocus carries.
 */
export function PlateScrim({
  onClose,
  className,
  children,
}: {
  readonly onClose: () => void;
  readonly className?: string;
  readonly children: ReactNode;
}) {
  return (
    <div
      className={`dialog__scrim${className === undefined ? '' : ` ${className}`}`}
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      {children}
    </div>
  );
}

/**
 * Trap focus inside the plate, close on Escape, and put focus back where it
 * came from when the plate leaves.
 */
export function usePlateFocus(
  panelRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  enabled = true,
): void {
  const closeRef = useRef(onClose);

  useEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!enabled) return;
    // Remembered before focus moves, so closing returns the user to the
    // control they opened this from rather than the top of the document.
    const opener = document.activeElement as HTMLElement | null;
    const panel = panelRef.current;
    const first = panel === null ? undefined : focusable(panel)[0];
    first?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        closeRef.current();
        return;
      }
      if (event.key !== 'Tab' || panel === null) {
        return;
      }
      // Trapped rather than merely started here: Tab out of a plate lands on
      // the page behind it, which is inert to the eye and not to the keyboard.
      const reachable = focusable(panel);
      const edge = event.shiftKey ? reachable[0] : reachable[reachable.length - 1];
      const wrap = event.shiftKey ? reachable[reachable.length - 1] : reachable[0];
      if (document.activeElement === edge && wrap !== undefined) {
        event.preventDefault();
        wrap.focus();
      }
    };

    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      opener?.focus();
    };
  }, [panelRef, enabled]);
}
