import { DesktopFeedback } from '../DesktopFeedback';
import { Button } from '../../ui/Button';
import { IconWarningOutline16 } from '../../ui/icons';
import type { DesktopStop } from '../desktopReconnect';

export function ComputerRecoveryPanel({
  stop,
  error,
  reconnectDisabled,
  onReconnect,
  onReturnToConversation,
}: {
  readonly stop?: DesktopStop;
  readonly error: string;
  readonly reconnectDisabled: boolean;
  readonly onReconnect: () => void;
  readonly onReturnToConversation?: () => void;
}) {
  return (
    <section className="computer-recovery-panel" aria-label="Computer view recovery">
      <div className="computer-recovery-panel__notice">
        <span aria-hidden="true">
          <IconWarningOutline16 size={24} />
        </span>
        <h3>
          {stop === 'exhausted'
            ? 'Automatic reconnect attempts have stopped'
            : stop === 'authorization'
              ? 'Computer access needs attention'
              : 'Computer view is unavailable'}
        </h3>
        <DesktopFeedback phase="error" error={error} notice={null} />
      </div>
      <div className="computer-recovery-panel__actions">
        <Button variant="primary" disabled={reconnectDisabled} onClick={onReconnect}>
          Reconnect view
        </Button>
        {onReturnToConversation ? (
          <Button onClick={onReturnToConversation}>Return to conversation</Button>
        ) : null}
      </div>
      <p>Reconnect opens the view only. It does not restart the Agent or take control.</p>
      <p>
        The Computer connection does not confirm the Agent’s current progress. Check the
        conversation for its recorded activity.
      </p>
    </section>
  );
}
