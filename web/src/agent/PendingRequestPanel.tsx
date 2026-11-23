import { useState } from 'react';
import { Button } from '../ui/Button';
import { CharacterAvatar } from '../character/CharacterAvatar';
import type { AgentController } from './controller';
import './AgentStatusPanel.css';

export function PendingRequestPanel({
  controller,
  unavailable,
  starting,
}: {
  readonly controller: Pick<
    AgentController,
    | 'pendingMessage'
    | 'pendingControl'
    | 'checkingInputReceipt'
    | 'pending'
    | 'inputAvailability'
    | 'recoverPending'
  >;
  readonly unavailable: boolean;
  readonly starting: boolean;
}) {
  const [copy, setCopy] = useState<'idle' | 'copying' | 'copied' | 'failed'>('idle');
  const message = controller.pendingMessage;
  const checking = controller.checkingInputReceipt;
  return (
    <section className="agent-pending-panel" aria-label="Pending request">
      <span className="agent-pending-panel__icon" aria-hidden="true">
        <CharacterAvatar pose="thinking" size={48} />
      </span>
      <div className="agent-pending-panel__body">
        <div role="alert">
          <h2>
            {checking
              ? 'Checking your message receipt'
              : message !== undefined
                ? 'We can’t confirm this message arrived'
                : 'We can’t confirm this response action'}
          </h2>
          <p>
            {checking
              ? 'Checking the receipt for your previous message.'
              : message !== undefined
                ? 'Your message is awaiting confirmation. Recover it before sending another message.'
                : 'Your response action is awaiting confirmation.'}
          </p>
          {!checking ? (
            <p>This doesn’t mean it failed. Recovery may send the same request again.</p>
          ) : null}
        </div>
        <div className="agent-pending-panel__actions">
          {checking && !controller.pendingControl ? null : (
            <Button
              variant="primary"
              size="sm"
              disabled={
                unavailable ||
                controller.pending !== undefined ||
                (starting && !controller.inputAvailability.allowed)
              }
              onClick={() => void controller.recoverPending?.()}
            >
              Recover pending request
            </Button>
          )}
          {message !== undefined ? (
            <Button
              variant="outline"
              size="sm"
              disabled={copy === 'copying'}
              onClick={() => {
                setCopy('copying');
                void (async () => {
                  try {
                    await navigator.clipboard.writeText(message);
                    setCopy('copied');
                  } catch {
                    setCopy('failed');
                  }
                })();
              }}
            >
              Copy message
            </Button>
          ) : null}
        </div>
        {copy === 'copied' ? <p role="status">Message copied.</p> : null}
        {copy === 'failed' ? (
          <div role="status">
            <p>Copy was unavailable. Select and copy your message below.</p>
            <textarea aria-label="Message to copy" readOnly value={message} />
          </div>
        ) : null}
      </div>
    </section>
  );
}
