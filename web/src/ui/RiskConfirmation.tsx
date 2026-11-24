import { Presence } from './Presence';
import { useState, type ReactNode } from 'react';
import { Button } from './Button';
import { Modal } from './Modal';
import { IconWarningOutline16 } from './icons';
import './RiskConfirmation.css';

/**
 * Confirming something that cannot be undone — the shared style contracts
 *
 * The safeguard is the tick, not the colour. A red button is a colour a
 * person can click as fast as any other; a checkbox is a second, deliberate
 * act, and the confirm key is unavailable until it happens.
 *
 * Used where the consequence is loss, plus §12.4's one bounded exception for
 * an ambiguous initial task that may execute twice. An ordinary action that
 * can be repeated or reversed asks in place of the row's own controls instead,
 * which is quieter and enough.
 *
 * The acknowledgement is held here rather than by the caller. Every caller
 * would otherwise reset it themselves on close, and one of them would forget
 * — leaving a dialog that opens already ticked, which is the one state this
 * control exists to prevent.
 */

interface RiskConfirmationProps {
  readonly open: boolean;
  readonly title: string;
  /** What will happen, in a sentence. */
  readonly description: ReactNode;
  /** The sentence beside the tick, in the first person. */
  readonly acknowledgeLabel: string;
  readonly confirmLabel: string;
  readonly cancelLabel?: string;
  readonly actionEmphasis?: 'confirm' | 'cancel';
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
  /** Something is already in flight; the confirm key waits for it. */
  readonly busy?: boolean;
  /** Current authority or revisions can prevent confirmation while keeping Cancel available. */
  readonly confirmDisabled?: boolean;
  /** An unresolved write cannot be dismissed until its same-key result is known. */
  readonly dismissible?: boolean;
  /** A command failure announced without replacing the consequence. */
  readonly failure?: string;
  /** Current command state, announced politely and held in the dialog. */
  readonly status?: string;
}

export function RiskConfirmation({
  open,
  title,
  description,
  acknowledgeLabel,
  confirmLabel,
  cancelLabel = 'Cancel',
  actionEmphasis = 'confirm',
  onCancel,
  onConfirm,
  busy = false,
  confirmDisabled = false,
  dismissible = true,
  failure,
  status,
}: RiskConfirmationProps) {
  const [acknowledged, setAcknowledged] = useState(false);

  const close = () => {
    if (!dismissible) return;
    setAcknowledged(false);
    onCancel();
  };

  return (
    <Modal
      open={open}
      title={title}
      placement={actionEmphasis === 'cancel' ? 'risk risk--retain' : 'risk'}
      dismissible={dismissible}
      onClose={close}
      footer={
        <div className="risk__actions">
          <Button
            variant={actionEmphasis === 'cancel' ? 'primary' : 'outline'}
            placement="risk__cancel"
            disabled={busy || !dismissible}
            onClick={close}
          >
            {cancelLabel}
          </Button>
          <Button
            variant={actionEmphasis === 'cancel' ? 'outline' : 'primary'}
            placement="risk__confirm"
            disabled={busy || confirmDisabled || !acknowledged}
            onClick={() => {
              setAcknowledged(false);
              onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </div>
      }
    >
      <div className="risk__warning" role="alert">
        <IconWarningOutline16 className="risk__icon" />
        <p>{description}</p>
      </div>
      <label className="risk__acknowledgement">
        <input
          type="checkbox"
          checked={acknowledged}
          disabled={busy}
          onChange={(event) => {
            setAcknowledged(event.target.checked);
          }}
        />
        {acknowledgeLabel}
      </label>
      <Presence>
        {status === undefined ? null : (
          <p className="risk__status" role="status" aria-live="polite">
            {status}
          </p>
        )}
      </Presence>
      <Presence>
        {failure === undefined ? null : (
          <p className="risk__failure" role="alert">
            {failure}
          </p>
        )}
      </Presence>
    </Modal>
  );
}
