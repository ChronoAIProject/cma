/**
 * The wire's own names, in one place.
 *
 * Every field name and JSON-Pointer path this SDK reads off the network is
 * declared here rather than spelled at its use site, and each one cites the
 * document that fixes it. Two defects found on 2026-08-19 were exactly this
 * knowledge being spelled twice and drifting: `STATE_DELTA`'s patches were
 * read from `patch` while the service sends `delta`, and usage was read from
 * `/usage/tokens` while the service writes `/usage/inputTokens`. Both dropped
 * real data in silence.
 *
 * A name that is wrong here is wrong once, fails every test that touches it,
 * and is fixed in one edit. That is the whole reason this file exists.
 */

/** Where an AG-UI event's RFC 6902 patches live — `docs/API_AGUI.md` §3. */
export const STATE_DELTA_PATCHES = 'delta';

/**
 * The JSON-Pointer paths a `STATE_DELTA` writes, as the service writes them.
 *
 * `docs/API_AGUI.md`: token counts arrive as patches against `/usage`, a
 * dropped stretch of conversation is appended to `/gaps`, and a plane change
 * is appended to `/planeChanges`.
 */
export const PATHS = {
  /** Tokens the turn consumed reading. */
  inputTokens: '/usage/inputTokens',
  /** Tokens the turn produced. */
  outputTokens: '/usage/outputTokens',
  /** How full the context is, as the runtime reports it. */
  contextUsed: '/usage/contextUsed',
  /** What the session has spent, where the runtime reports a figure. */
  cost: '/usage/cost',
  /** The permission mode the agent is running under. */
  mode: '/mode',
  /** What the agent intends to do, as steps. */
  plan: '/plan',
  /** The settings this session may change, in the kernel's own words. */
  configOptions: '/configOptions',
  /** Where a plane change is appended. */
  planeChanges: '/planeChanges/-',
  /** Where a stretch of lost conversation is appended. */
  gaps: '/gaps/-',
} as const;

/**
 * The event kinds this build knows.
 *
 * A kind absent from this list is not dropped — `docs/API_AGUI.md` says the
 * set grows additively, so an unfamiliar kind is carried whole and shown as
 * something this build cannot draw yet, never as nothing.
 */
export const KNOWN_EVENTS = [
  'RUN_STARTED',
  'RUN_FINISHED',
  'RUN_ERROR',
  'TEXT_MESSAGE_START',
  'TEXT_MESSAGE_CONTENT',
  'TEXT_MESSAGE_END',
  'TOOL_CALL_START',
  'TOOL_CALL_ARGS',
  'TOOL_CALL_END',
  'TOOL_CALL_RESULT',
  'REASONING_MESSAGE_CONTENT',
  'STATE_DELTA',
] as const;

/**
 * The marker a frame carries when it is liveness rather than record.
 *
 * `docs/API_AGUI.md`: the live lane is what the agent is saying right now and
 * survives no reload. It carries no sequence of its own — what it arrives
 * with is whatever the last durable frame carried — so it must never touch a
 * client's cursor or its seen-set.
 */
export const LIVE_MARKER = 'chrono:live';

/**
 * How a client tells one delivered frame from another.
 *
 * The durable sequence alone is not enough, and assuming it was is what made
 * every agent message render empty: one durable event legitimately projects
 * to several frames — a committed message becomes START, then CONTENT, then
 * END — and all of them carry the sequence of the event they came from,
 * because that sequence is the cursor a reconnect resumes on. No projection
 * emits the same kind twice for one event, so the pair is exact.
 */
export function frameKey(seq: number, type: string): string {
  return `${String(seq)}:${type}`;
}
