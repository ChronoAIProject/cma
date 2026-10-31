import { CharacterAvatar } from '../character/CharacterAvatar';
import { characterForStatus, type ResourceStatus } from '../character/status';
import './ConversationEmpty.css';

/**
 * A conversation before its first message (§28): the Agent's character, large and awake,
 * over the one line that says what to do. Held and failed Agents stay still.
 */
export function ConversationEmpty({
  message,
  status = 'ready',
}: {
  readonly message: string;
  readonly status?: ResourceStatus;
}) {
  const presence = characterForStatus(status);
  return (
    <div className="conversation-empty">
      <CharacterAvatar
        pose={presence.pose}
        tone={presence.tone}
        still={presence.pose === 'paused' || presence.pose === 'failed'}
        shadow
        size={120}
      />
      <p className="conversation-empty__message">{message}</p>
    </div>
  );
}
