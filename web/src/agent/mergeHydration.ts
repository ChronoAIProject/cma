import type { AgentLiveEvent } from '../api/types';
import { foldLiveEvent } from './conversation';
import type { ConversationState } from './model';

interface BufferedEvent {
  readonly event: AgentLiveEvent;
  /** The stream's last committed event, independent of a newer HTTP snapshot. */
  readonly afterSequence: number;
}

function messageSequence(event: AgentLiveEvent): number | undefined {
  if (event['cma:live'] === true || typeof event.messageId !== 'string') return undefined;
  const match = /^msg_(\d+)$/.exec(event.messageId);
  return match === null ? undefined : Number(match[1]);
}

function applies(
  snapshot: ConversationState | undefined,
  current: ConversationState,
  { event, afterSequence }: BufferedEvent,
) {
  if (snapshot === undefined) return true;
  // Only the last frame of a durable AG-UI entry carries its SSE cursor.
  // Message ids identify the entire START/CONTENT/END group, including a group
  // whose END frame has not arrived when HTTP hydration catches up.
  const sequence = messageSequence(event) ?? event.cursor;
  if (sequence !== undefined) return sequence > snapshot.cursor;
  // Auxiliary stream state (for example navigation metadata) need not belong
  // to an active turn; durable state frames were checked by cursor above.
  if (event.type === 'STATE_DELTA') return true;
  const turn = Number(event.runId);
  const newerTurn = Number.isSafeInteger(turn) && turn > snapshot.cursor;
  if (event.type === 'RUN_STARTED')
    return newerTurn || (!Number.isSafeInteger(turn) && afterSequence >= snapshot.cursor);
  if (event['cma:live'] === true) {
    if (current.activeRunId !== event.runId) return false;
    if (!newerTurn && snapshot.activeRunId !== event.runId) return false;
    return !(
      event.type === 'TEXT_MESSAGE_CONTENT' &&
      snapshot.messages.some(
        (message) =>
          message.role === 'assistant' &&
          message.runId === event.runId &&
          (message.sequence ?? 0) > afterSequence,
      )
    );
  }
  // An unsequenced RUN_FINISHED precedes an approval's sequenced STATE_DELTA.
  // A snapshot may already contain that approval (including its resolution).
  if (event.type === 'RUN_FINISHED') {
    const outcome = event.outcome as { interrupts?: readonly { id?: string }[] } | undefined;
    if (
      outcome?.interrupts?.length &&
      outcome.interrupts.every((item) => snapshot.approvals.some((known) => known.id === item.id))
    )
      return false;
  }
  return newerTurn || current.activeRunId === event.runId;
}

/** Keeps new live output across hydration without replaying already recorded work. */
export class HydrationEvents {
  private snapshot?: ConversationState;
  private events: BufferedEvent[] = [];
  private streamCursor = 0;

  reset(snapshot?: ConversationState) {
    this.snapshot = snapshot;
    this.events = [];
    this.streamCursor = snapshot?.cursor ?? 0;
  }

  replayFrom(cursor: number) {
    this.streamCursor = cursor;
  }

  apply(state: ConversationState, event: AgentLiveEvent, agent: string, buffering: boolean) {
    if (event.threadId !== agent) return state;
    const entry = { event, afterSequence: this.streamCursor };
    this.streamCursor = Math.max(this.streamCursor, event.cursor ?? 0);
    if (!applies(this.snapshot, state, entry)) return state;
    if (buffering) {
      const previous = this.events.at(-1);
      // Stream fragments can be numerous; retain their text, not one object per token.
      if (
        event['cma:live'] === true &&
        typeof event.delta === 'string' &&
        previous?.event['cma:live'] === true &&
        previous.event.type === event.type &&
        previous.event.runId === event.runId &&
        previous.event.messageId === event.messageId &&
        previous.afterSequence === entry.afterSequence &&
        typeof previous.event.delta === 'string'
      )
        this.events[this.events.length - 1] = {
          ...entry,
          event: { ...event, delta: previous.event.delta + event.delta },
        };
      else this.events.push(entry);
    }
    return foldLiveEvent(state, event, agent);
  }

  merge(snapshot: ConversationState, agent: string, complete: boolean) {
    this.snapshot = snapshot;
    let conversation = snapshot;
    this.events = this.events.filter((entry) => {
      if (!applies(snapshot, conversation, entry)) return false;
      conversation = foldLiveEvent(conversation, entry.event, agent);
      return true;
    });
    if (complete) this.events = [];
    return conversation;
  }
}
