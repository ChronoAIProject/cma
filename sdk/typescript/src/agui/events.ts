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

import { KNOWN_EVENTS, STATE_DELTA_PATCHES } from '../wire.js';
import { ProtocolError } from '../errors.js';
import { canBeDurableProjectionPrefix, isCursorlessInterfaceBoundary } from './cursor.js';

/** Every kind this build renders. The set grows; the handling does not break. */
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
   *
   * Zero has three explicit meanings, none of which advances the cursor: a
   * leading durable projection frame whose final carries the positive
   * sequence, liveness distinguished by `ephemeral`, or the non-live
   * `RUN_STARTED` boundary that opens a continuation interface.
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
  /** `confirmation`, or `cma:<the runtime's own word>`. */
  readonly reason: string;
  /** The kernel's own wording, or `null` when it supplied none. */
  readonly message: string | null;
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

/** A tool call's arguments, when the session records tool bodies. */
export interface ToolCallArgs extends EventBase {
  readonly type: 'TOOL_CALL_ARGS';
  readonly toolCallId: string;
  readonly delta: string;
  /** Whether the body was cut to fit the log's bound. */
  readonly truncated: boolean;
}

/** A tool call's result, when the session records tool bodies. */
export interface ToolCallResult extends EventBase {
  readonly type: 'TOOL_CALL_RESULT';
  readonly toolCallId: string;
  readonly content: string;
  /** Whether the body was cut to fit the log's bound. */
  readonly truncated: boolean;
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
  readonly from?: string;
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
  /**
   * Stable identity of this frame within one durable projection.
   *
   * Assigned by the projection assembler after the numbered final frame
   * reveals the projection's durable sequence. Both `AguiConnection` and
   * `parseEventStream` use that assembler. This is client metadata, not wire
   * data; directly constructed events may omit it for backwards compatibility.
   */
  readonly projectionId?: string;
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
  | ToolCallArgs
  | ToolCallEnd
  | ToolCallResult
  | ReasoningContent
  | StateDelta
  | UnknownEvent;

/** The first of these that is a string, if any is. */
function firstString(...values: readonly unknown[]): string | undefined {
  return values.find((value): value is string => typeof value === 'string');
}

/**
 * Read one frame off the wire.
 *
 * Unknown discriminators survive with their complete payload. Malformed known
 * frames fail as `ProtocolError`: forward compatibility is not permission to
 * silently invent missing identifiers, content, or patches.
 */
export function toEvent(raw: unknown): AguiEvent {
  const frame = objectOf(raw, 'AG-UI frame');
  const ephemeral = frame['cma:live'] === true;
  if (frame['cma:live'] !== undefined && typeof frame['cma:live'] !== 'boolean') {
    throw new ProtocolError("AG-UI frame 'cma:live' was not boolean");
  }
  const kind = nonEmptyString(frame.type, 'AG-UI type');
  const seq = sequence(frame.seq);
  if (
    !ephemeral &&
    seq === 0 &&
    !canBeDurableProjectionPrefix(kind) &&
    !isCursorlessInterfaceBoundary(kind, seq, ephemeral)
  ) {
    throw new ProtocolError(`AG-UI durable ${kind} frame cannot have sequence zero`);
  }
  const base = {
    threadId: nonEmptyString(frame.threadId, 'AG-UI threadId'),
    runId: nonEmptyString(frame.runId, 'AG-UI runId'),
    seq,
    timestamp: optionalString(frame.timestamp, 'AG-UI timestamp'),
    ephemeral,
  };

  switch (kind) {
    case 'RUN_STARTED':
      return { ...base, type: 'RUN_STARTED' };
    case 'RUN_FINISHED':
      return { ...base, type: 'RUN_FINISHED', outcome: outcome(frame.outcome) };
    case 'RUN_ERROR':
      optionalString(frame.capability, 'RUN_ERROR capability');
      optionalString(frame.code, 'RUN_ERROR code');
      optionalBoolean(frame['cma:returns'], "RUN_ERROR 'cma:returns'");
      return {
        ...base,
        type: 'RUN_ERROR',
        message: requiredString(frame.message, 'RUN_ERROR message'),
        capability: typeof frame.capability === 'string' ? frame.capability : undefined,
        code: typeof frame.code === 'string' ? frame.code : undefined,
        // Read under the extension prefix the document reserves. Absent on a
        // control plane that predates it, which reads as `false` — a failure,
        // which is the safer of the two to be wrong about: a handover drawn
        // as a failure is confusing, and a failure drawn as a handover tells
        // somebody their session is coming back when it is not.
        returns: frame['cma:returns'] === true,
      };
    case 'TEXT_MESSAGE_START':
      if (frame.role !== 'user' && frame.role !== 'assistant') {
        throw new ProtocolError("TEXT_MESSAGE_START role was not 'user' or 'assistant'");
      }
      return {
        ...base,
        type: 'TEXT_MESSAGE_START',
        messageId: nonEmptyString(frame.messageId, 'TEXT_MESSAGE_START messageId'),
        role: frame.role,
      };
    case 'TEXT_MESSAGE_CONTENT':
      return {
        ...base,
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: nonEmptyString(frame.messageId, 'TEXT_MESSAGE_CONTENT messageId'),
        delta: requiredString(frame.delta, 'TEXT_MESSAGE_CONTENT delta'),
      };
    case 'TEXT_MESSAGE_END':
      return {
        ...base,
        type: 'TEXT_MESSAGE_END',
        messageId: nonEmptyString(frame.messageId, 'TEXT_MESSAGE_END messageId'),
      };
    case 'TOOL_CALL_START':
      optionalString(frame.toolCallName, 'TOOL_CALL_START toolCallName');
      optionalString(frame.target, 'TOOL_CALL_START target');
      return {
        ...base,
        type: 'TOOL_CALL_START',
        toolCallId: nonEmptyString(frame.toolCallId, 'TOOL_CALL_START toolCallId'),
        // The kernel's own word, verbatim (§3). A vocabulary this build has
        // not heard of is rendered as it arrived rather than mapped.
        toolKind: nonEmptyString(
          frame['cma:toolKind'] ?? frame.toolKind,
          "TOOL_CALL_START 'cma:toolKind'",
        ),
        target: firstString(frame.toolCallName, frame.target),
      };
    case 'TOOL_CALL_ARGS':
      requiredBoolean(frame['cma:truncated'], "TOOL_CALL_ARGS 'cma:truncated'");
      return {
        ...base,
        type: 'TOOL_CALL_ARGS',
        toolCallId: nonEmptyString(frame.toolCallId, 'TOOL_CALL_ARGS toolCallId'),
        delta: requiredString(frame.delta, 'TOOL_CALL_ARGS delta'),
        truncated: frame['cma:truncated'] === true,
      };
    case 'TOOL_CALL_END':
      optionalString(frame.output, 'TOOL_CALL_END output');
      return {
        ...base,
        type: 'TOOL_CALL_END',
        toolCallId: nonEmptyString(frame.toolCallId, 'TOOL_CALL_END toolCallId'),
        status: nonEmptyString(frame['cma:status'] ?? frame.status, "TOOL_CALL_END 'cma:status'"),
        output: typeof frame.output === 'string' ? frame.output : undefined,
      };
    case 'TOOL_CALL_RESULT':
      requiredBoolean(frame['cma:truncated'], "TOOL_CALL_RESULT 'cma:truncated'");
      return {
        ...base,
        type: 'TOOL_CALL_RESULT',
        toolCallId: nonEmptyString(frame.toolCallId, 'TOOL_CALL_RESULT toolCallId'),
        content: requiredString(frame.content, 'TOOL_CALL_RESULT content'),
        truncated: frame['cma:truncated'] === true,
      };
    case 'REASONING_MESSAGE_CONTENT':
      return {
        ...base,
        type: 'REASONING_MESSAGE_CONTENT',
        messageId: nonEmptyString(frame.messageId, 'REASONING_MESSAGE_CONTENT messageId'),
        delta: requiredString(frame.delta, 'REASONING_MESSAGE_CONTENT delta'),
      };
    case 'STATE_DELTA':
      /*
       * Read from the name the service actually uses. This said `frame.patch`
       * and the service sends `delta`, so every STATE_DELTA arrived carrying
       * an empty patch list and was applied to nothing: no session ever
       * showed what it had spent. The name lives in `wire.ts` now, spelled
       * once.
       */
      return {
        ...base,
        type: 'STATE_DELTA',
        patch: patches(frame[STATE_DELTA_PATCHES]),
      };
    default:
      return { ...base, type: 'UNKNOWN', kind, raw: frame };
  }
}

function outcome(raw: unknown): RunOutcome | undefined {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const value = objectOf(raw, 'RUN_FINISHED outcome');
  const type = nonEmptyString(value.type, 'RUN_FINISHED outcome type');
  if (type !== 'interrupt') {
    return { type: 'other', kind: type };
  }
  if (!Array.isArray(value.interrupts)) {
    throw new ProtocolError('interrupt outcome carried no interrupt list');
  }
  return { type: 'interrupt', interrupts: value.interrupts.map(interrupt) };
}

function interrupt(raw: unknown): Interrupt {
  const value = objectOf(raw, 'interrupt');
  const options = value.options;
  if (options !== undefined && !Array.isArray(options)) {
    throw new ProtocolError('interrupt options were not a list');
  }
  return {
    id: nonEmptyString(value.id, 'interrupt id'),
    reason: nonEmptyString(value.reason, 'interrupt reason'),
    message: requiredNullableString(value.message, 'interrupt message'),
    responseSchema: value.responseSchema,
    expiresAt: optionalString(value.expiresAt, 'interrupt expiresAt'),
    options: options?.map(option),
  };
}

function option(raw: unknown): InterruptOption {
  const value = objectOf(raw, 'interrupt option');
  optionalString(value.label, 'interrupt option label');
  optionalBoolean(value.affirmative, 'interrupt option affirmative');
  return {
    decision: nonEmptyString(value.decision, 'interrupt option decision'),
    label: typeof value.label === 'string' ? value.label : undefined,
    affirmative: value.affirmative === true,
  };
}

function patches(raw: unknown): readonly JsonPatch[] {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`STATE_DELTA ${STATE_DELTA_PATCHES} was not a list`);
  }
  return raw.map(patch);
}

function patch(raw: unknown): JsonPatch {
  const value = objectOf(raw, 'STATE_DELTA patch');
  const op = nonEmptyString(requiredPatchMember(value, 'op'), 'STATE_DELTA patch op');
  // The empty string is RFC 6902's pointer to the document root.
  const path = requiredString(requiredPatchMember(value, 'path'), 'STATE_DELTA patch path');

  switch (op) {
    case 'add':
    case 'replace':
    case 'test':
      return { op, path, value: requiredPatchValue(value) };
    case 'copy':
    case 'move':
      return {
        op,
        path,
        from: requiredString(requiredPatchMember(value, 'from'), 'STATE_DELTA patch from'),
      };
    case 'remove':
      return { op, path };
    default:
      throw new ProtocolError(`STATE_DELTA patch used unsupported operation '${op}'`);
  }
}

function requiredPatchMember(patch: Record<string, unknown>, member: string): unknown {
  if (!Object.prototype.hasOwnProperty.call(patch, member)) {
    throw new ProtocolError(`STATE_DELTA patch ${member} was missing`);
  }
  return patch[member];
}

function requiredPatchValue(patch: Record<string, unknown>): unknown {
  if (!Object.prototype.hasOwnProperty.call(patch, 'value') || patch.value === undefined) {
    throw new ProtocolError('STATE_DELTA patch value was missing');
  }
  return patch.value;
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function requiredString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not a string`);
  }
  return raw;
}

function requiredNullableString(raw: unknown, name: string): string | null {
  if (raw === null) {
    return null;
  }
  return requiredString(raw, name);
}

function nonEmptyString(raw: unknown, name: string): string {
  const value = requiredString(raw, name);
  if (value === '') {
    throw new ProtocolError(`${name} was empty`);
  }
  return value;
}

function optionalString(raw: unknown, name: string): string | undefined {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  return requiredString(raw, name);
}

function requiredBoolean(raw: unknown, name: string): boolean {
  if (typeof raw !== 'boolean') {
    throw new ProtocolError(`${name} was not boolean`);
  }
  return raw;
}

function optionalBoolean(raw: unknown, name: string): void {
  if (raw !== undefined && raw !== null && typeof raw !== 'boolean') {
    throw new ProtocolError(`${name} was not boolean`);
  }
}

function sequence(raw: unknown): number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw < 0) {
    throw new ProtocolError('AG-UI seq was not a non-negative safe integer');
  }
  return raw;
}
