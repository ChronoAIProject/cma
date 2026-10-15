import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { IconCloseOutline16 } from './icons';
import { Button } from './Button';
import { usePlateFocus } from './plate';
import { Presence, presenceAttributes } from './Presence';
import './Modal.css';

/**
 * The product's one dialog — `docs/Web_Style.md` §12.4.
 *
 * Portalled to `document.body`, which is not a detail: an ancestor that
 * establishes a stacking context leaves sticky page chrome floating above the
 * mask, and no z-index on the dialog can reach past it.
 *
 * The focus trap, the return of focus on close and the Escape route are ours
 * rather than the design's — they are the parts nobody notices are missing
 * until a keyboard user is stranded behind a dialog they cannot leave. This
 * repository had two dialog implementations once and one of them trapped
 * nothing, which is why the machinery lives in `plate.ts` and is called from
 * exactly one place now.
 *
 * **No dialog opens over a dialog.** A surface that must ask something while
 * one is open asks in place of the row's own controls — which is what a
 * destructive confirmation does.
 */

interface ModalProps {
  readonly open: boolean;
  readonly onClose: () => void;
  /** The heading, and the dialog's accessible name in every mode. */
  readonly title: string;
  /** A supporting sentence under the title. */
  readonly description?: string;
  readonly children?: ReactNode;
  /** The action row. The primary action goes last. */
  readonly footer?: ReactNode;
  /** Whether Escape, the close key, and the mask may dismiss this dialog. */
  readonly dismissible?: boolean;
  /**
   * Draw the card and nothing else.
   *
   * For a surface whose own header is part of what it is showing. The mask,
   * the card, Escape and the accessible name all remain — only the default
   * header, close key and body padding are dropped.
   */
  readonly headless?: boolean;
  /** Additional surface class; card sizing and placement remain shared. */
  readonly placement?: string;
}

export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  dismissible = true,
  headless = false,
  placement,
}: ModalProps) {
  const panelRef = useRef<HTMLDivElement>(null);

  return (
    <Presence>
      {open ? (
        <ModalBody
          onClose={onClose}
          title={title}
          description={description}
          footer={footer}
          dismissible={dismissible}
          headless={headless}
          placement={placement}
          panelRef={panelRef}
        >
          {children}
        </ModalBody>
      ) : null}
    </Presence>
  );
}

/**
 * Split from `Modal` so the focus hook runs only while the dialog is open.
 *
 * Hooks cannot be called conditionally, and a trap armed on a closed dialog
 * would steal focus from the page behind it.
 */
function ModalBody({
  onClose,
  title,
  description,
  children,
  footer,
  dismissible,
  headless,
  placement,
  panelRef,
  'data-presence': presence,
}: Omit<ModalProps, 'open' | 'headless' | 'dismissible'> & {
  readonly headless: boolean;
  readonly dismissible: boolean;
  readonly panelRef: React.RefObject<HTMLDivElement | null>;
  readonly 'data-presence'?: 'visible' | 'exiting';
}) {
  const exiting = presence === 'exiting';
  const close = () => {
    if (dismissible && !exiting) onClose();
  };
  usePlateFocus(panelRef, close, !exiting);

  useEffect(() => {
    if (exiting) return;
    /*
     * The page behind a modal does not scroll. Without this the mask stays
     * put while the page moves under it, which reads as the dialog sliding.
     */
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previous;
    };
  }, [exiting]);

  return createPortal(
    <div className="modal" role="presentation" {...presenceAttributes(exiting)}>
      <div className="modal__mask" aria-hidden="true" onClick={close} />
      <div
        className={['modal__card', placement ?? ''].filter(Boolean).join(' ')}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        ref={panelRef}
      >
        {headless ? (
          children
        ) : (
          <>
            <div className="modal__content">
              <div className="modal__header">
                <h2 className="modal__title">{title}</h2>
                <Button
                  size="sm"
                  placement="modal__close"
                  aria-label="Close"
                  icon={<IconCloseOutline16 size={14} />}
                  disabled={!dismissible}
                  onClick={close}
                />
              </div>
              {description === undefined || description === '' ? null : (
                <p className="modal__description">{description}</p>
              )}
              {children === undefined ? null : <div className="modal__body">{children}</div>}
            </div>
            {footer === undefined ? null : <div className="modal__footer">{footer}</div>}
          </>
        )}
      </div>
    </div>,
    document.body,
  );
}
