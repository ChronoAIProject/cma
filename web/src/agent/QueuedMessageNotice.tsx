import { Button } from '../ui/Button';
import type { QueuedMessageControl } from './transport/queuedStatus';

export function QueuedMessageNotice({
  control,
  readOnly,
}: {
  readonly control?: QueuedMessageControl;
  readonly readOnly: boolean;
}) {
  if (control === undefined) return null;
  return (
    <>
      {control.error === undefined ? null : (
        <div className="agent-action-error" role="alert">
          {control.error}
        </div>
      )}
      {control.notice === undefined ? null : (
        <div className="agent-action-error" role="status">
          <span>
            {control.notice}
            {control.message === undefined
              ? ''
              : ` ${control.message.length > 160 ? `${control.message.slice(0, 160)}…` : control.message}`}
          </span>
          {readOnly ? null : control.needsRecovery ? (
            <Button size="sm" disabled={control.busy} onClick={() => void control.recover()}>
              Recover cancellation
            </Button>
          ) : control.message === undefined ? null : (
            <Button size="sm" disabled={!control.canCancel} onClick={() => void control.cancel()}>
              Cancel queued message
            </Button>
          )}
        </div>
      )}
    </>
  );
}
