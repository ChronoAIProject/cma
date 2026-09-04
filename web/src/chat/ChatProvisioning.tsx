import type { FailureNotice } from '../sessions/useSessionActions';
import { LandingCharacter } from '../landing/LandingCharacter';
import { Button } from '../ui/Button';
import { IconRefreshOutline16, IconWarningOutline16 } from '../ui/icons';
import './ChatProvisioning.css';

interface ChatProvisioningProps {
  readonly state:
    | { readonly kind: 'preparing'; readonly message: string }
    | { readonly kind: 'failed'; readonly notice: FailureNotice };
  readonly onRetry: () => void;
}

/** The new Chat seat shown before the control plane has returned a session id. */
export function ChatProvisioning({ state, onRetry }: ChatProvisioningProps) {
  if (state.kind === 'failed') {
    return (
      <div className="chat-provisioning chat-provisioning--failed">
        <div className="chat-provisioning__failure" role="alert">
          <IconWarningOutline16 className="chat-provisioning__warning" />
          <h1>Chat could not be prepared</h1>
          <p>{state.notice.failed}</p>
          {state.notice.unchanged === undefined ? null : <p>{state.notice.unchanged}</p>}
          {state.notice.code === undefined ? null : (
            <code className="chat-provisioning__code">{state.notice.code}</code>
          )}
          <Button variant="primary" icon={<IconRefreshOutline16 />} onClick={onRetry}>
            Try again
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="chat-provisioning" aria-busy="true">
      <LandingCharacter className="chat-provisioning__character" mode="provisioning" />
      <div className="chat-provisioning__status" role="status" aria-live="polite">
        <h1>Preparing your Chat</h1>
        <p>{state.message}</p>
      </div>
    </div>
  );
}
