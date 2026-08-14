import { useEffect, useId, useRef } from 'react';
import type { ReactNode } from 'react';
import './Dialog.css';

/**
 * The product's one overlay, per docs/Web_Style.md section 6.
 *
 * Shared rather than reimplemented per surface, because the parts that are
 * easy to get wrong — the focus trap, the return of focus on close, Escape,
 * the scrim click — are the parts nobody notices are missing until a keyboard
 * user is stranded behind a dialog they cannot leave.
 */

interface DialogProps {
  readonly title: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  /** Actions for the footer. The accent action goes last. */
  readonly footer?: ReactNode;
}

/** Everything inside `root` that a Tab can reach. */
function focusable(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  );
}

export function Dialog({ title, onClose, children, footer }: DialogProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    // Remembered before focus moves, so closing returns the user to the
    // control they opened this from rather than the top of the document.
    const opener = document.activeElement as HTMLElement | null;
    const panel = panelRef.current;
    const first = panel === null ? undefined : focusable(panel)[0];
    first?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key !== 'Tab' || panel === null) {
        return;
      }
      // Trapped rather than merely started here: Tab out of a modal lands on
      // the page behind it, which is still inert to the eye and not to the
      // keyboard.
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
  }, [onClose]);

  return (
    <div
      className="dialog__scrim"
      // Dismissing on the scrim is a pointer affordance; the keyboard route is
      // Escape, which is why this carries no role and no handler of its own
      // beyond the click.
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        ref={panelRef}
      >
        <header className="dialog__header">
          <h2 className="dialog__title" id={titleId}>
            {title}
          </h2>
          <button type="button" className="dialog__close" onClick={onClose}>
            <span aria-hidden="true">×</span>
            <span className="sr-only">Close</span>
          </button>
        </header>

        <div className="dialog__body">{children}</div>

        {footer === undefined ? null : <footer className="dialog__footer">{footer}</footer>}
      </div>
    </div>
  );
}
