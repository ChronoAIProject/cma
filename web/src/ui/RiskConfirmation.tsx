import { useState } from 'react';
import { Button } from './Button';
import { Modal } from './Modal';
import { IconWarningOutline16 } from './icons';
import './RiskConfirmation.css';

/**
 * Confirming something that cannot be undone — `docs/Web_Style.md` §7.
 *
 * The safeguard is the tick, not the colour. A red button is a colour a
 * person can click as fast as any other; a checkbox is a second, deliberate
 * act, and the confirm key is unavailable until it happens.
 *
 * Used where the consequence is loss rather than merely change — deleting a
 * session and its history. An action that can be repeated or reversed asks in
 * place of the row's own controls instead, which is quieter and enough.
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
  readonly description: string;
  /** The sentence beside the tick, in the first person. */
  readonly acknowledgeLabel: string;
  readonly confirmLabel: string;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
  /** Something is already in flight; the confirm key waits for it. */
  readonly busy?: boolean;
}

export function RiskConfirmation({
  open,
  title,
  description,
  acknowledgeLabel,
  confirmLabel,
  onCancel,
  onConfirm,
  busy = false,
}: RiskConfirmationProps) {
  const [acknowledged, setAcknowledged] = useState(false);

  const close = () => {
    setAcknowledged(false);
    onCancel();
  };

  return (
    <Modal
      open={open}
      title={title}
      placement="risk"
      onClose={close}
      footer={
        <>
          <Button variant="outline" placement="risk__cancel" onClick={close}>
            Cancel
          </Button>
          <Button
            variant="primary"
            placement="risk__confirm"
            disabled={busy || !acknowledged}
            onClick={() => {
              setAcknowledged(false);
              onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="risk__warning">
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
    </Modal>
  );
}
