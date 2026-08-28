import { Button } from './Button';
import './InlineConfirm.css';

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
  readonly cancelDisabled?: boolean;
}

export function InlineConfirm({
  question,
  confirmLabel,
  onConfirm,
  onCancel,
  busy = false,
  cancelDisabled = false,
}: ConfirmProps) {
  return (
    <div className="confirm" role="group">
      <span className="confirm__question">{question}</span>
      <Button size="sm" disabled={cancelDisabled} onClick={onCancel}>
        Keep it
      </Button>
      <Button size="sm" variant="primary" disabled={busy} onClick={onConfirm}>
        {confirmLabel}
      </Button>
    </div>
  );
}
