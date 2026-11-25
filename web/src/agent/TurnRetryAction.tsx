import type { ConversationState, RecoveryNotice } from './model';
import { Button } from '../ui/Button';
import { turnRetryInput } from './turnRetryInput';

export function TurnRetryAction({
  conversation,
  notice,
  onPrepare,
}: {
  readonly conversation: ConversationState;
  readonly notice: RecoveryNotice;
  readonly onPrepare?: (input: string) => void;
}) {
  if (
    notice.runId === undefined ||
    !['agent_turn_timeout', 'approval_timeout'].includes(notice.technicalDetails?.category ?? '')
  )
    return null;
  const retry = turnRetryInput(conversation, notice.runId);
  if (retry === undefined) return null;
  if ('unavailable' in retry) return <p>{retry.unavailable}</p>;
  return onPrepare ? (
    <Button size="sm" variant="outline" onClick={() => onPrepare(retry.input)}>
      Review and retry
    </Button>
  ) : null;
}
