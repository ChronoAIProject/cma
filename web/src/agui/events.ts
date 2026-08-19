/**
 * The AG-UI event vocabulary, transcribed from `docs/API_AGUI.md` §3.
 *
 * Discriminated on `type`, so a kind this build does not handle is a type
 * error at the point of handling rather than a silent drop at runtime.
 *
 * **An unknown kind must survive.** The emitted set grows additively — the
 * document says so in as many words — and a client that discarded what it did
 * not recognise would lose the one thing a newer control plane was trying to
 * tell it. Unknown kinds are carried through as `UnknownEvent` with their
 * payload intact, which is the same property the durable log holds server-side.
 */

/** Every kind this build renders. The set grows; the handling does not break. */
export const KNOWN_EVENTS = [
  'RUN_STARTED',
  'RUN_FINISHED',
  'RUN_ERROR',
  'TEXT_MESSAGE_START',
  'TEXT_MESSAGE_CONTENT',
  'TEXT_MESSAGE_END',
  'TOOL_CALL_START',
  'TOOL_CALL_END',
  'REASONING_MESSAGE_CONTENT',
  'STATE_DELTA',
] as const;

export type KnownEventType = (typeof KNOWN_EVENTS)[number];

/** What every event carries, whether or not this build knows its kind. */
interface EventBase {
  /**
   * The session. `docs/API_AGUI.md` §2: a run against a new thread returns the
   * identifier the service chose rather than the one that was sent, so this is
   * read off the event rather than assumed from the request.
   */
  readonly threadId: string;
  readonly runId: string;
  /**
   * Per-session monotonic sequence, and the only resumption state there is.
   *
   * `API_Overview.md` §3: durable events carry a per-session `seq`, **not a
   * global one**. A client that treated it as global would lose events in
   * exactly the window nobody tests.
   */
  readonly seq: number;
  /**
   * Whether this frame is what is happening rather than what happened.
   *
   * A liveness frame — a piece of a message the agent is still typing — is not
   * in the durable log and carries no cursor. It **must not** touch the
   * resumption state: `seq` is meaningless on one, and treating it as a
   * position would send the next reconnect somewhere the log has never been.
   *
   * Stated by the server rather than inferred. `EventSource` reports
   * `lastEventId` as the last id it was *given*, so a frame sent without one
   * is indistinguishable here from a frame repeating the previous id — and
   * deduplicating on that would drop every liveness frame in silence.
   */
  readonly ephemeral?: boolean;
  /**
   * When the durable event this was projected from was appended, RFC 3339.
   *
   * Absent for liveness (which has no log position) and for events a control
   * plane predating timestamps wrote. §18.1 renders no time then — never an
   * estimated one.
   */
  readonly timestamp?: string;
}

export interface RunStarted extends EventBase {
  readonly type: 'RUN_STARTED';
}

/** How a run ended. An interrupt outcome means the session is alive. */
export interface RunFinished extends EventBase {
  readonly type: 'RUN_FINISHED';
  readonly outcome?: RunOutcome;
}

/**
 * A piece of the agent's reasoning, as it reasons.
 *
 * Always liveness — this never appears in the durable log, so a reload shows
 * none of it. `REASONING_*` is AG-UI's current family; `THINKING_*` is the
 * deprecated spelling of the same thing and is not emitted.
 */
export interface ReasoningContent extends EventBase {
  readonly type: 'REASONING_MESSAGE_CONTENT';
  readonly messageId: string;
  readonly delta: string;
}

export interface RunError extends EventBase {
  readonly type: 'RUN_ERROR';
  readonly message: string;
  /**
   * The capability this session lacks, when that is why it failed.
   *
   * `API_Overview.md` rule 4: errors name what is missing. The name reaches
   * the surface, because an interface that renders it as "something went
   * wrong" has thrown away the only part a person could act on.
   */
  readonly capability?: string;
  /** The stop reason, as the API spells it. */
  readonly code?: string;
  /**
   * Whether the session is coming back.
   *
   * A handover — onto a replacement runtime or onto the other plane — ends
   * the run, because the runtime behind it is being torn down and a client
   * left on a stream that simply stops cannot tell that from a slow agent.
   * Nothing has gone wrong, though, and `Web_Style.md` §11 forbids a rotating
   * session taking the failure ink. The protocol has one event for a run
   * ending badly, so this is what a client distinguishes on.
   */
  readonly returns?: boolean;
}

/**
 * How a run ended.
 *
 * Two members rather than one open string, because a client makes exactly one
 * decision from this: `docs/API_AGUI.md` §7 — "do not treat `RUN_FINISHED` as
 * session over; check `outcome`. An interrupt outcome means the session is
 * alive and waiting for you." Anything else keeps the kernel's own word, which
 * is shown but never branched on.
 */
export type RunOutcome =
  | { readonly type: 'interrupt'; readonly interrupts: readonly Interrupt[] }
  | { readonly type: 'other'; readonly kind: string };

/**
 * A question the kernel stopped to ask.
 *
 * The offered answers come from the kernel and are rendered as they arrived —
 * never a hardcoded allow/deny pair, because the sets differ per kernel and
 * grow (`docs/API_AGUI.md` §4).
 */
export interface Interrupt {
  readonly id: string;
  /** `confirmation`, or `chrono:<the runtime's own word>`. */
  readonly reason: string;
  /** The kernel's own wording of what it wants. Rendered verbatim. */
  readonly message: string;
  readonly responseSchema?: unknown;
  readonly expiresAt?: string;
  /**
   * The answers this kernel offered, when it said.
   *
   * Absent means the kernel offered none by name, and the client falls back to
   * the two the protocol always defines — never to a set it invented.
   */
  readonly options?: readonly InterruptOption[];
}

export interface InterruptOption {
  /** The value sent back as `payload.decision`. */
  readonly decision: string;
  /** What the key says. Falls back to the decision when the kernel gave none. */
  readonly label?: string;
  /** Whether choosing this is the affirmative answer. */
  readonly affirmative?: boolean;
}

export interface TextMessageStart extends EventBase {
  readonly type: 'TEXT_MESSAGE_START';
  readonly messageId: string;
  readonly role: 'assistant' | 'user';
}

/**
 * The finished message.
 *
 * `docs/API_AGUI.md` §3: text arrives whole rather than as tokens. The
 * token-level pieces exist and travel as liveness on interfaces holding a live
 * connection, and are deliberately not in the durable log — numbering them
 * would punch permanent holes in the sequence a resuming client cursors on.
 */
export interface TextMessageContent extends EventBase {
  readonly type: 'TEXT_MESSAGE_CONTENT';
  readonly messageId: string;
  readonly delta: string;
}

export interface TextMessageEnd extends EventBase {
  readonly type: 'TEXT_MESSAGE_END';
  readonly messageId: string;
}

export interface ToolCallStart extends EventBase {
  readonly type: 'TOOL_CALL_START';
  readonly toolCallId: string;
  /** The kernel's own word for what kind of tool this is. Never mapped. */
  readonly toolKind: string;
  /**
   * What it is acting on, when the kernel said.
   *
   * Read from `toolCallName`, which is the protocol's field. This client read
   * `target` — a name nothing sends — so every tool call arrived without the
   * one thing that identifies it, and §18.2's collapsed row could only ever
   * say a kind and a status. `target` is still accepted, so a client of an
   * older build is not broken by the correction.
   */
  readonly target?: string;
}

export interface ToolCallEnd extends EventBase {
  readonly type: 'TOOL_CALL_END';
  readonly toolCallId: string;
  /** The kernel's own status word. Never mapped onto a fixed set. */
  readonly status: string;
  readonly output?: string;
}

/**
 * An RFC 6902 patch against the run's state.
 *
 * `docs/API_AGUI.md` §3: usage lands at `/usage` and dropped conversation at
 * `/gaps`. Patches rather than replaces.
 */
export interface StateDelta extends EventBase {
  readonly type: 'STATE_DELTA';
  readonly patch: readonly JsonPatch[];
}

export interface JsonPatch {
  readonly op: string;
  readonly path: string;
  readonly value?: unknown;
}

/**
 * A kind this build does not know.
 *
 * Kept whole rather than dropped. The set grows additively and a client that
 * threw one away would silently lose what a newer control plane was reporting.
 */
export interface UnknownEvent extends EventBase {
  readonly type: 'UNKNOWN';
  /** The kind as it arrived, so it can be shown and reported. */
  readonly kind: string;
  /** The frame verbatim, so nothing is lost in the round trip. */
  readonly raw: Readonly<Record<string, unknown>>;
}

export type AguiEvent =
  | RunStarted
  | RunFinished
  | RunError
  | TextMessageStart
  | TextMessageContent
  | TextMessageEnd
  | ToolCallStart
  | ToolCallEnd
  | ReasoningContent
  | StateDelta
  | UnknownEvent;

/** The first of these that is a string, if any is. */
function firstString(...values: readonly unknown[]): string | undefined {
  return values.find((value): value is string => typeof value === 'string');
}

function str(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

function num(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

/**
 * Read one frame off the wire.
 *
 * Never throws and never returns nothing: a frame this build cannot classify
 * becomes `UnknownEvent` carrying its payload. This is a trust boundary, and
 * the honest failure here is "we do not know what this is", not a crash and
 * not silence.
 */
export function toEvent(raw: unknown): AguiEvent {
  const frame = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  const base = {
    threadId: str(frame.threadId),
    runId: str(frame.runId),
    seq: num(frame.seq),
    timestamp: typeof frame.timestamp === 'string' ? frame.timestamp : undefined,
    ephemeral: frame['chrono:live'] === true,
  };
  const kind = str(frame.type);

  switch (kind) {
    case 'RUN_STARTED':
      return { ...base, type: 'RUN_STARTED' };
    case 'RUN_FINISHED':
      return { ...base, type: 'RUN_FINISHED', outcome: outcome(frame.outcome) };
    case 'RUN_ERROR':
      return {
        ...base,
        type: 'RUN_ERROR',
        message: str(frame.message, 'the run failed'),
        capability: typeof frame.capability === 'string' ? frame.capability : undefined,
        code: typeof frame.code === 'string' ? frame.code : undefined,
        // Read under the extension prefix the document reserves. Absent on a
        // control plane that predates it, which reads as `false` — a failure,
        // which is the safer of the two to be wrong about: a handover drawn
        // as a failure is confusing, and a failure drawn as a handover tells
        // somebody their session is coming back when it is not.
        returns: frame['chrono:returns'] === true,
      };
    case 'TEXT_MESSAGE_START':
      return {
        ...base,
        type: 'TEXT_MESSAGE_START',
        messageId: str(frame.messageId),
        role: frame.role === 'user' ? 'user' : 'assistant',
      };
    case 'TEXT_MESSAGE_CONTENT':
      return {
        ...base,
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: str(frame.messageId),
        delta: str(frame.delta),
      };
    case 'TEXT_MESSAGE_END':
      return { ...base, type: 'TEXT_MESSAGE_END', messageId: str(frame.messageId) };
    case 'TOOL_CALL_START':
      return {
        ...base,
        type: 'TOOL_CALL_START',
        toolCallId: str(frame.toolCallId),
        // The kernel's own word, verbatim (§3). A vocabulary this build has
        // not heard of is rendered as it arrived rather than mapped.
        toolKind: str(frame['chrono:toolKind'] ?? frame.toolKind, 'tool'),
        target: firstString(frame.toolCallName, frame.target),
      };
    case 'TOOL_CALL_END':
      return {
        ...base,
        type: 'TOOL_CALL_END',
        toolCallId: str(frame.toolCallId),
        status: str(frame['chrono:status'] ?? frame.status, 'finished'),
        output: typeof frame.output === 'string' ? frame.output : undefined,
      };
    case 'REASONING_MESSAGE_CONTENT':
      return {
        ...base,
        type: 'REASONING_MESSAGE_CONTENT',
        messageId: str(frame.messageId),
        delta: str(frame.delta),
      };
    case 'STATE_DELTA':
      return { ...base, type: 'STATE_DELTA', patch: patches(frame.patch) };
    default:
      return { ...base, type: 'UNKNOWN', kind, raw: frame };
  }
}

function outcome(raw: unknown): RunOutcome | undefined {
  if (typeof raw !== 'object' || raw === null) {
    return undefined;
  }
  const value = raw as Record<string, unknown>;
  const type = str(value.type);
  if (type !== 'interrupt') {
    return type === '' ? undefined : { type: 'other', kind: type };
  }
  const list = Array.isArray(value.interrupts) ? value.interrupts : [];
  return { type: 'interrupt', interrupts: list.flatMap(interrupt) };
}

function interrupt(raw: unknown): Interrupt[] {
  if (typeof raw !== 'object' || raw === null) {
    return [];
  }
  const value = raw as Record<string, unknown>;
  const id = str(value.id);
  if (id === '') {
    return [];
  }
  return [
    {
      id,
      reason: str(value.reason, 'confirmation'),
      message: str(value.message),
      responseSchema: value.responseSchema,
      expiresAt: typeof value.expiresAt === 'string' ? value.expiresAt : undefined,
      options: Array.isArray(value.options) ? value.options.flatMap(option) : undefined,
    },
  ];
}

function option(raw: unknown): InterruptOption[] {
  if (typeof raw !== 'object' || raw === null) {
    return [];
  }
  const value = raw as Record<string, unknown>;
  const decision = str(value.decision);
  if (decision === '') {
    return [];
  }
  return [
    {
      decision,
      label: typeof value.label === 'string' ? value.label : undefined,
      affirmative: value.affirmative === true,
    },
  ];
}

function patches(raw: unknown): readonly JsonPatch[] {
  if (!Array.isArray(raw)) {
    return [];
  }
  return raw.flatMap((entry): JsonPatch[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const value = entry as Record<string, unknown>;
    const path = str(value.path);
    return path === '' ? [] : [{ op: str(value.op, 'replace'), path, value: value.value }];
  });
}
