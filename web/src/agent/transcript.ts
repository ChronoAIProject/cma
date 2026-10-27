import type { AgentTranscript } from '../api/types';
import type { ConversationMessage, ConversationState, RecoveryNotice } from './model';
import { object, text } from './conversationValues';

function messageStatus(outcome: Readonly<Record<string, unknown>>) {
  return outcome.type === 'failed' ? 'error' : 'complete';
}

export function conversationFromTranscript(transcript: AgentTranscript): ConversationState {
  const messages = new Map<string, ConversationMessage>();
  for (const turn of transcript.turns) {
    for (const message of turn.messages) {
      const id = `event:${String(message.sequence)}`;
      // A queued pre-write retry keeps the admitted event but belongs to the latest turn.
      messages.set(id, {
        id,
        runId: String(turn.turnSequence),
        role: message.role,
        content: message.content,
        status: message.role === 'user' ? 'complete' : messageStatus(turn.outcome),
        at: message.appendedAt,
        order: message.sequence,
        sequence: message.sequence,
      });
    }
  }
  const activeTurn = transcript.turns.find((turn) => turn.outcome.type === 'active');
  const recovery: RecoveryNotice[] = transcript.turns.flatMap((turn) => {
    const failure = object(turn.outcome.failure);
    if (turn.outcome.type !== 'failed') return [];
    return [
      {
        id: `turn-error:${String(turn.turnSequence)}`,
        runId: String(turn.turnSequence),
        afterSequence: Math.max(
          turn.turnSequence,
          ...turn.messages.map((message) => message.sequence),
        ),
        title: 'Agent run failed',
        detail: text(failure?.detail) ?? 'The Agent run did not complete.',
        code: text(failure?.class),
      },
    ];
  });
  return {
    messages: [...messages.values()].sort((left, right) => left.order! - right.order!),
    toolCalls: [],
    reasoning: [],
    approvals: [],
    plans: [],
    recovery,
    activeRunId: activeTurn === undefined ? undefined : String(activeTurn.turnSequence),
    activeRunStartedAt: activeTurn?.startedAt,
    turnTiming: Object.fromEntries(
      transcript.turns.map((turn) => [
        String(turn.turnSequence),
        {
          startedAt: turn.startedAt,
          finishedAt: turn.outcome.type === 'active' ? undefined : text(turn.outcome.at),
        },
      ]),
    ),
    cursor: transcript.throughSeq,
  };
}
