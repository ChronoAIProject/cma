import type { AguiEvent, Interrupt } from '../agui';

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
 */

/** One thing in the stream, in the order it happened. */
export type Entry = MessageEntry | ToolCallEntry | GapEntry | UnknownEntry;

export interface MessageEntry {
  readonly kind: 'message';
  readonly id: string;
  readonly role: 'assistant' | 'user';
  readonly text: string;
  /** False while the turn is still in flight. Carries the caret; nothing else. */
  readonly committed: boolean;
  readonly seq: number;
}

export interface ToolCallEntry {
  readonly kind: 'tool';
  readonly id: string;
  /** The kernel's own word. Never mapped onto a set this client invented. */
  readonly toolKind: string;
  readonly target?: string;
  /** Absent while it is still running, which is how §18.2 tells them apart. */
  readonly status?: string;
  readonly output?: string;
  readonly seq: number;
}

/**
 * A boundary where conversation was lost.
 *
 * §18.6 and `CLAUDE.md` §5: anything the agent did after the snapshot did not
 * travel, and an owner who can see the boundary can recognise a gap rather
 * than puzzle over one.
 */
export interface GapEntry {
  readonly kind: 'gap';
  readonly id: string;
  readonly rotatedAt?: string;
  readonly reason?: string;
  readonly seq: number;
}

/** An event kind this build does not know. Shown, never dropped. */
export interface UnknownEntry {
  readonly kind: 'unknown';
  readonly id: string;
  readonly eventKind: string;
  readonly seq: number;
}

/** Everything a surface needs to draw the conversation. */
export interface Conversation {
  readonly entries: readonly Entry[];
  /** True between `RUN_STARTED` and the `RUN_FINISHED` that closes it. */
  readonly turnInFlight: boolean;
  /** The approval the session is parked on, when it is parked on one. */
  readonly pending?: Interrupt;
  /**
   * What the session has cost, when it reports it.
   *
   * `undefined` is not zero (§18.5): a session with no `usage` grant reports
   * nothing, and has often spent a great deal.
   */
  readonly usage?: Usage;
  /** Why the last run failed, when one did. */
  readonly error?: { readonly message: string; readonly capability?: string };
}

export interface Usage {
  readonly tokens?: number;
  readonly cost?: string;
}

export const EMPTY: Conversation = { entries: [], turnInFlight: false };

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
      return { ...state, turnInFlight: true, pending: undefined, error: undefined };

    case 'RUN_FINISHED': {
      const pending = event.outcome?.type === 'interrupt' ? event.outcome.interrupts[0] : undefined;
      return {
        ...state,
        turnInFlight: false,
        // Committed on the way out: a message whose turn ended is settled
        // whether or not its own end frame arrived.
        entries: state.entries.map((entry) =>
          entry.kind === 'message' ? { ...entry, committed: true } : entry,
        ),
        pending,
      };
    }

    case 'RUN_ERROR':
      return {
        ...state,
        turnInFlight: false,
        error: { message: event.message, capability: event.capability },
      };

    case 'TEXT_MESSAGE_START':
      return upsertMessage(state, {
        kind: 'message',
        id: event.messageId,
        role: event.role,
        text: '',
        committed: false,
        seq: event.seq,
      });

    case 'TEXT_MESSAGE_CONTENT': {
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
      });
    }

    case 'TEXT_MESSAGE_END': {
      const existing = state.entries.find(
        (entry): entry is MessageEntry => entry.kind === 'message' && entry.id === event.messageId,
      );
      return existing === undefined
        ? state
        : upsertMessage(state, { ...existing, committed: true });
    }

    case 'TOOL_CALL_START':
      return append(state, {
        kind: 'tool',
        id: event.toolCallId,
        toolKind: event.toolKind,
        target: event.target,
        seq: event.seq,
      });

    case 'TOOL_CALL_END':
      return {
        ...state,
        entries: state.entries.map((entry) =>
          entry.kind === 'tool' && entry.id === event.toolCallId
            ? { ...entry, status: event.status, output: event.output }
            : entry,
        ),
      };

    case 'STATE_DELTA':
      return applyPatches(state, event);

    case 'UNKNOWN':
      // Shown rather than dropped. The emitted set grows additively, and what
      // a newer control plane is reporting is worth more than a tidy stream.
      return append(state, {
        kind: 'unknown',
        id: `unknown-${String(event.seq)}`,
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
export function fold(events: Iterable<AguiEvent>, from: Conversation = EMPTY): Conversation {
  let state = from;
  for (const event of events) {
    state = reduce(state, event);
  }
  return state;
}

function append(state: Conversation, entry: Entry): Conversation {
  if (state.entries.some((existing) => existing.kind === entry.kind && existing.id === entry.id)) {
    return state;
  }
  return { ...state, entries: [...state.entries, entry] };
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

/**
 * Apply the patches a `STATE_DELTA` carries.
 *
 * Two paths matter to this surface and the rest are ignored rather than
 * guessed at: `/usage` is what a session has cost (§18.5) and `/gaps` is where
 * a stretch of lost conversation lands (§18.6).
 */
function applyPatches(
  state: Conversation,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;
  for (const patch of event.patch) {
    if (patch.path === '/usage/tokens' && typeof patch.value === 'number') {
      next = { ...next, usage: { ...next.usage, tokens: patch.value } };
    } else if (patch.path === '/usage/cost' && typeof patch.value === 'string') {
      next = { ...next, usage: { ...next.usage, cost: patch.value } };
    } else if (patch.path.startsWith('/gaps')) {
      const value = (
        typeof patch.value === 'object' && patch.value !== null ? patch.value : {}
      ) as Record<string, unknown>;
      next = append(next, {
        kind: 'gap',
        id: `gap-${String(event.seq)}`,
        rotatedAt: typeof value.rotatedAt === 'string' ? value.rotatedAt : undefined,
        reason: typeof value.reason === 'string' ? value.reason : undefined,
        seq: event.seq,
      });
    }
  }
  return next;
}
