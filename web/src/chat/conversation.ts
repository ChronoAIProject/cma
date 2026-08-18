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
export type Entry = MessageEntry | ToolCallEntry | GapEntry | FailureEntry | UnknownEntry;

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
  /**
   * Where the session was moved to, when the boundary is a plane change.
   *
   * The same boundary either way — history stopped being continuous at
   * `rotatedAt` — and worth naming, because an owner who asked for the move is
   * looking for confirmation that this gap is the one they caused rather than
   * a second thing that went wrong.
   */
  readonly movedTo?: string;
  readonly seq: number;
}

/**
 * A run that stopped badly — §18.9.
 *
 * In the stream rather than in a band above it, because a failure has a
 * position: it happened after the last tool call and before nothing. §18.6
 * already settles this for the other kind of boundary, and a failure hoisted
 * out of the stream loses the one thing that says what the agent was doing
 * when it stopped.
 *
 * A rotation never becomes one of these. It ends the run and reaches this
 * client on the same frame, but nothing has gone wrong and §11 forbids it
 * taking the failure ink.
 */
export interface FailureEntry {
  readonly kind: 'failure';
  readonly id: string;
  /** What failed, in the kernel's own words. */
  readonly message: string;
  /**
   * The ability the session lacked, when the failure was a refusal.
   *
   * Named rather than described. An interface that renders this as "something
   * went wrong" has thrown away the only part a person could act on.
   */
  readonly capability?: string;
  /** The stop reason, as the API spells it. */
  readonly code?: string;
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
  /**
   * What the agent is saying right now, before it has committed to it.
   *
   * **Deliberately not an entry.** §18.1 requires a turn drawn from history
   * and a turn watched arriving to produce the same result, and history has
   * no in-flight message: a reload folds no live frames at all and therefore
   * has no live slot, so the two agree by construction. Making it an entry
   * and remembering to remove it would be the same rule enforced by
   * discipline, which is weaker.
   *
   * Replaced, not appended to, by the committed message. They are two
   * identities — the in-flight one has no sequence, because it has no place
   * in the log yet — so the turn is never drawn twice.
   */
  readonly live?: { readonly text: string };
  /**
   * What the agent is reasoning about, while it reasons — §18.8.
   *
   * Held outside `entries` for the same reason `live` is, and more
   * emphatically: a message is eventually committed and a thought never is.
   * Nothing here is in the durable log, so a reload draws none of it and the
   * equivalence rule holds without anything having to be remembered.
   *
   * A reader who saw reasoning and reloaded to find it gone will otherwise
   * report data loss, which is why §18.8 says so on the surface rather than
   * only here.
   */
  readonly reasoning?: { readonly text: string };
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
      return {
        ...state,
        turnInFlight: true,
        pending: undefined,
        error: undefined,
        live: undefined,
        reasoning: undefined,
      };

    case 'RUN_FINISHED': {
      const pending = event.outcome?.type === 'interrupt' ? event.outcome.interrupts[0] : undefined;
      return {
        ...state,
        turnInFlight: false,
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
      if (event.returns) {
        return { ...state, turnInFlight: false, live: undefined, reasoning: undefined };
      }
      return {
        ...state,
        turnInFlight: false,
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
      return upsertMessage({ ...state, live: undefined }, {
        kind: 'message',
        id: event.messageId,
        role: event.role,
        text: '',
        committed: false,
        seq: event.seq,
      });

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

    case 'REASONING_MESSAGE_CONTENT':
      /*
       * Never an entry, and never cleared by anything except the run ending.
       * A thought is not committed, so there is no committed counterpart for
       * it to be replaced by — which is the one way it differs from the
       * message being typed.
       */
      return { ...state, reasoning: { text: (state.reasoning?.text ?? '') + event.delta } };

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

/**
 * Apply the patches a `STATE_DELTA` carries.
 *
 * Three paths matter to this surface and the rest are ignored rather than
 * guessed at: `/usage` is what a session has cost (§18.5), `/gaps` is where a
 * stretch of lost conversation lands, and `/planeChanges` is where a session
 * moved between planes says so (§18.6).
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
    } else if (patch.path.startsWith('/planeChanges')) {
      /*
       * §18.6: drawn as a rule across the stream at the snapshot moment, not
       * as a status chip. Anything the agent did after that moment did not
       * travel, and a gap somebody can see is one they can recognise.
       */
      const value = (
        typeof patch.value === 'object' && patch.value !== null ? patch.value : {}
      ) as Record<string, unknown>;
      next = append(next, {
        kind: 'gap',
        id: `plane-${String(event.seq)}`,
        rotatedAt: typeof value.snapshotAt === 'string' ? value.snapshotAt : undefined,
        movedTo: typeof value.to === 'string' ? value.to : undefined,
        seq: event.seq,
      });
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
