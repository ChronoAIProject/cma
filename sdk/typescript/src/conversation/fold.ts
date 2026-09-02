import type { AguiEvent } from '../agui/index.js';
import { initialConversation, type Conversation, type Entry, type MessageEntry } from './model.js';
import { reduceStateDelta } from './state.js';

/**
 * The conversation, folded out of the event stream — `Web_Style.md` §18.1.
 *
 * A pure reducer, deliberately. The rule §18.1 turns on is that **a turn drawn
 * from history and a turn watched arriving must produce the same result**, and
 * the only way to hold that is for both to go through one function that cannot
 * tell which it is looking at. A component that folded live frames one way and
 * replayed frames another would have two answers, and the one a person reloads
 * into is the one they will believe.
 *
 * No React and no DOM: what a conversation *is* is decided here, and how it is
 * drawn is decided by the component that renders this.
 *
 * The shape itself lives in `conversationModel.ts` and is re-exported here.
 * Every surface on this plane asks this module for both, and the split is
 * about this file's length rather than about a boundary worth making callers
 * learn.
 */

export * from './model.js';

/**
 * Fold one event into the conversation.
 *
 * Order-independent in the one way that matters: an event already folded is
 * folded again to the same result, so the deduplication the connection does on
 * `seq` and the reducer here cannot disagree about what happened.
 */
export function reduce(state: Conversation, event: AguiEvent): Conversation {
  switch (event.type) {
    case 'RUN_STARTED':
      // A new run clears the answered approval and the last failure; both
      // belonged to the run that ended.
      return {
        ...state,
        turnInFlight: true,
        pending: undefined,
        error: undefined,
        live: undefined,
        reasoning: undefined,
      };

    case 'RUN_FINISHED': {
      /*
       * The approval this run stopped on, with the moment it stopped.
       *
       * The frame's own timestamp, not a clock read here: §18.1's equivalence
       * rule means a turn replayed from history must produce what a turn
       * watched arriving produced, and `Date.now()` on reload would produce
       * the time of the reload.
       */
      const asked = event.outcome?.type === 'interrupt' ? event.outcome.interrupts[0] : undefined;
      const pending = asked === undefined ? undefined : { ...asked, askedAt: event.timestamp };
      return {
        ...state,
        turnInFlight: false,
        sessionInteractive: false,
        // The turn is over, so nothing is in flight. A message that was being
        // typed has been committed by now and is in the stream; one that was
        // not is gone with the turn, and history has no record of it either —
        // which is the equivalence §18.1 asks for.
        live: undefined,
        // And the thinking that led to it. §18.8: it is liveness, it is
        // removed when the turn ends, and a reload shows none of it.
        reasoning: undefined,
        // Committed on the way out: a message whose turn ended is settled
        // whether or not its own end frame arrived.
        entries: state.entries.map((entry) =>
          entry.kind === 'message' ? { ...entry, committed: true } : entry,
        ),
        pending,
      };
    }

    case 'RUN_ERROR': {
      /*
       * A session being carried onto a replacement runtime, or onto the other
       * plane, ends the run on this same frame: its runtime is being torn
       * down, and a client left holding a stream that simply stops cannot
       * tell that from an agent taking its time. Nothing has gone wrong, so
       * it is neither drawn nor recorded as a failure — §11 forbids a
       * rotating session taking the failure ink, and §18.6 already says how
       * the boundary itself is drawn. The turn really has ended either way.
       */
      // Cursorless RUN_ERROR is also the shape used for admission refusal and
      // does not report a lifecycle edge. Durable projections come from a
      // noninteractive SessionStatus and definitively end current pressure.
      const lifecycleEnded = event.seq > 0;
      if (event.returns) {
        return {
          ...state,
          turnInFlight: false,
          sessionInteractive: false,
          cpuPressure: lifecycleEnded ? undefined : state.cpuPressure,
          memoryPressure: lifecycleEnded ? undefined : state.memoryPressure,
          live: undefined,
          reasoning: undefined,
        };
      }
      return {
        ...state,
        turnInFlight: false,
        sessionInteractive: false,
        cpuPressure: lifecycleEnded ? undefined : state.cpuPressure,
        memoryPressure: lifecycleEnded ? undefined : state.memoryPressure,
        live: undefined,
        reasoning: undefined,
        error: { message: event.message, capability: event.capability },
        // Also an entry, so it is drawn where it happened. `error` alone is
        // what this client had, and nothing read it.
        entries: appendOnce(state.entries, {
          kind: 'failure',
          id: `failure-${String(event.seq)}`,
          message: event.message,
          capability: event.capability,
          code: event.code,
          seq: event.seq,
        }),
      };
    }

    case 'TEXT_MESSAGE_START':
      /*
       * The committed message arriving is what clears the live slot. The two
       * are different identities, so leaving it would draw the turn twice —
       * once as it was typed and once as it was recorded.
       */
      return upsertMessage(
        { ...state, live: undefined },
        {
          kind: 'message',
          id: event.messageId,
          role: event.role,
          text: '',
          committed: false,
          seq: event.seq,
          at: event.timestamp,
        },
      );

    case 'TEXT_MESSAGE_CONTENT': {
      /*
       * A piece of a message still being typed. It goes to the live slot
       * rather than into the stream, and never touches an entry: the message
       * it belongs to has no sequence yet, so there is nothing in the stream
       * for it to belong to.
       */
      if (event.ephemeral === true) {
        return { ...state, live: { text: (state.live?.text ?? '') + event.delta } };
      }
      const existing = state.entries.find(
        (entry): entry is MessageEntry => entry.kind === 'message' && entry.id === event.messageId,
      );
      return upsertMessage(state, {
        kind: 'message',
        id: event.messageId,
        role: existing?.role ?? 'assistant',
        // Appended rather than replaced: the durable log carries the message
        // whole, and a live stream may carry it in pieces. Appending is
        // correct for both, and is what makes the two agree.
        text: (existing?.text ?? '') + event.delta,
        committed: existing?.committed ?? false,
        seq: existing?.seq ?? event.seq,
        at: existing?.at ?? event.timestamp,
      });
    }

    case 'TEXT_MESSAGE_END': {
      const existing = state.entries.find(
        (entry): entry is MessageEntry => entry.kind === 'message' && entry.id === event.messageId,
      );
      return existing === undefined
        ? state
        : upsertMessage(state, {
            ...existing,
            committed: true,
            // The END frame is the projection's resume point. Leading START
            // and CONTENT frames are deliberately unnumbered.
            seq: event.seq,
          });
    }

    case 'TOOL_CALL_START':
      return append(state, {
        kind: 'tool',
        id: event.toolCallId,
        toolKind: event.toolKind,
        target: event.target,
        seq: event.seq,
        startedAt: event.timestamp,
      });

    case 'TOOL_CALL_ARGS':
      return {
        ...state,
        entries: state.entries.map((entry) =>
          entry.kind === 'tool' && entry.id === event.toolCallId
            ? {
                ...entry,
                arguments: { text: event.delta, truncated: event.truncated },
                // ARGS closes the started projection when a body was recorded.
                seq: event.seq,
              }
            : entry,
        ),
      };

    case 'TOOL_CALL_RESULT':
      return {
        ...state,
        entries: state.entries.map((entry) =>
          entry.kind === 'tool' && entry.id === event.toolCallId
            ? { ...entry, result: { text: event.content, truncated: event.truncated } }
            : entry,
        ),
      };

    case 'TOOL_CALL_END':
      return {
        ...state,
        entries: state.entries.map((entry) =>
          entry.kind === 'tool' && entry.id === event.toolCallId
            ? { ...entry, status: event.status, output: event.output, finishedAt: event.timestamp }
            : entry,
        ),
      };

    case 'REASONING_MESSAGE_CONTENT':
      /*
       * Never an entry, and never cleared by anything except the run ending.
       * A thought is not committed, so there is no committed counterpart for
       * it to be replaced by — which is the one way it differs from the
       * message being typed.
       */
      return { ...state, reasoning: { text: (state.reasoning?.text ?? '') + event.delta } };

    case 'STATE_DELTA':
      return reduceStateDelta(state, event);

    case 'UNKNOWN':
      // Shown rather than dropped. The emitted set grows additively, and what
      // a newer control plane is reporting is worth more than a tidy stream.
      return append(state, {
        kind: 'unknown',
        id: event.projectionId ?? `unknown-${String(event.seq)}`,
        eventKind: event.kind,
        seq: event.seq,
      });

    default: {
      const exhaustive: never = event;
      return exhaustive;
    }
  }
}

/** Fold a whole stream. What history and a live connection both go through. */
export function fold(
  events: Iterable<AguiEvent>,
  from: Conversation = initialConversation(),
): Conversation {
  let state = from;
  for (const event of events) {
    state = reduce(state, event);
  }
  return state;
}

function append(state: Conversation, entry: Entry): Conversation {
  return { ...state, entries: appendOnce(state.entries, entry) };
}

/**
 * Add an entry unless one of that kind and id is already there.
 *
 * The idempotence the whole reducer turns on: an event folded twice — which
 * the deduplication above cannot rule out across a reconnect — must not
 * produce two entries.
 */
function appendOnce(entries: readonly Entry[], entry: Entry): readonly Entry[] {
  if (entries.some((existing) => existing.kind === entry.kind && existing.id === entry.id)) {
    return entries;
  }
  return [...entries, entry];
}

function upsertMessage(state: Conversation, message: MessageEntry): Conversation {
  const at = state.entries.findIndex(
    (entry) => entry.kind === 'message' && entry.id === message.id,
  );
  if (at < 0) {
    return { ...state, entries: [...state.entries, message] };
  }
  const entries = [...state.entries];
  entries[at] = message;
  return { ...state, entries };
}
