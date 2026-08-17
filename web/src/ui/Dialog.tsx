import { useEffect, useId, useRef } from 'react';
import type { ReactNode } from 'react';
import { Button } from './Button';
import './Dialog.css';

/**
 * The product's one dialog — docs/Web_Style.md §3 and §7.
 *
 * A **plate**: it floats over the page, takes the card impression, and never
 * moves. §3 lists a plate among the things that carry 5px of impression and
 * among the things that do not press in on hover.
 *
 * Shared rather than reimplemented per surface, because the parts that are
 * easy to get wrong — the focus trap, the return of focus on close, Escape,
 * the scrim click — are the parts nobody notices are missing until a keyboard
 * user is stranded behind a dialog they cannot leave. There were two
 * implementations of this; one of them trapped nothing.
 *
 * **No dialog opens over a dialog.** A surface that needs to ask something
 * while a dialog is open asks inline, in place of the row's own controls (§7),
 * which is also what a destructive confirmation does.
 */

/** The three widths §7 allows a dialog. Nothing else. */
export type DialogWidth = 'narrow' | 'medium' | 'wide';

interface DialogProps {
  readonly title: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  /**
   * How wide the plate is.
   *
   * 520 for a question, 700 for a form, 1000 for a form with a picker beside
   * it. Three and no others: a width chosen per surface is how two dialogs
   * that ask the same kind of question end up different sizes.
   */
  readonly width?: DialogWidth;
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

export function Dialog({ title, onClose, children, width = 'narrow', footer }: DialogProps) {
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
        className={`dialog dialog--${width}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        ref={panelRef}
      >
        <header className="band band--head dialog__header">
          <h2 className="dialog__title" id={titleId}>
            {title}
          </h2>
          <span className="band__spacer" />
          <button type="button" className="collapse-handle dialog__close" onClick={onClose}>
            <span aria-hidden="true">×</span>
            <span className="sr-only">Close</span>
          </button>
        </header>

        <div className="dialog__body">{children}</div>

        {footer === undefined ? null : (
          <footer className="band band--foot dialog__footer">{footer}</footer>
        )}
      </div>
    </div>
  );
}

/**
 * A destructive question asked in place of the controls that raised it — §7.
 *
 * Exported beside the dialog because it is the answer to "how do I confirm
 * something while a dialog is open": never a second dialog over the first.
 */
interface ConfirmProps {
  readonly question: string;
  readonly confirmLabel: string;
  readonly onConfirm: () => void;
  readonly onCancel: () => void;
  readonly busy?: boolean;
}

export function InlineConfirm({
  question,
  confirmLabel,
  onConfirm,
  onCancel,
  busy = false,
}: ConfirmProps) {
  return (
    <div className="confirm" role="group">
      <span className="confirm__question">{question}</span>
      <Button size="sm" onClick={onCancel}>
        Keep it
      </Button>
      <Button size="sm" tone="danger" disabled={busy} onClick={onConfirm}>
        {confirmLabel}
      </Button>
    </div>
  );
}
