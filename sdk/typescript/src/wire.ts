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

/** Exact session-status spellings serialized by Layer 2 on REST and AG-UI. */
export const SESSION_STATUSES = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
] as const;

export type SessionStatus = (typeof SESSION_STATUSES)[number];

/** Exact provisioning-stage spellings serialized by Layer 2 on AG-UI. */
export const PROVISIONING_STAGES = [
  'accepted',
  'runtime_requested',
  'runtime_created',
  'skills_installed',
  'attaching',
  'ready',
] as const;

export type ProvisioningStage = (typeof PROVISIONING_STAGES)[number];

/** The durable state value for one approval the session is waiting on. */
export interface PendingApprovalState {
  readonly id: string;
  readonly reason: string;
  readonly message: string | null;
}

/** The public document an AG-UI session's first `STATE_DELTA` patches. */
export interface AguiState {
  /** Additive protocol fields a newer control plane may introduce. */
  [key: string]: unknown;
  usage: {
    /** Additive usage fields a newer control plane may introduce. */
    [key: string]: unknown;
    inputTokens: number | null;
    outputTokens: number | null;
    contextUsed: number | null;
    contextWindow: number | null;
  };
  status: SessionStatus | null;
  progress: ProvisioningStage | null;
  mode: unknown;
  plan: unknown;
  configOptions: unknown;
  availableCommands: unknown[] | null;
  pendingApprovals: Record<string, PendingApprovalState>;
  signIn: unknown;
  gaps: unknown[];
  planeChanges: unknown[];
  contextCompactions: unknown[];
  runtimeLosses: unknown[];
  memoryPressure: unknown;
  resources: unknown;
}

/**
 * Return a fresh copy of `docs/API_AGUI.md` §3's canonical initial state.
 *
 * JSON Patch implementations commonly mutate their target. A factory keeps
 * one session's fold from becoming the next session's starting document.
 */
export function initialState(): AguiState {
  return {
    usage: {
      inputTokens: null,
      outputTokens: null,
      contextUsed: null,
      contextWindow: null,
    },
    status: null,
    progress: null,
    mode: null,
    plan: null,
    configOptions: null,
    availableCommands: null,
    pendingApprovals: {},
    signIn: null,
    gaps: [],
    planeChanges: [],
    contextCompactions: [],
    runtimeLosses: [],
    memoryPressure: null,
    resources: null,
  };
}

/** The one field in a `/gaps/-` value, as emitted by `agui/project.rs`. */
export const GAP_VALUE_FRAMES = 'frames';

/**
 * Pricing names on the REST wire — `docs/API_REST.md` §7.
 *
 * Kept together before the SDK starts consuming the quote in the confirmation
 * flow: the create claim, quote response, and pinned session projection share
 * names, and spelling any of them locally recreates the drift this file exists
 * to prevent.
 */
export const PRICING_WIRE = {
  quotePath: '/api/v1/pricing/quote',
  spend: 'spend',
  environmentProfile: 'environment_profile',
  quotedRateCard: 'quoted_rate_card',
  pricing: 'pricing',
  priced: 'priced',
  rateCard: 'rate_card',
  currency: 'currency',
  cpuMillis: 'cpu_millis',
  memoryMib: 'memory_mib',
  microsPerHour: 'micros_per_hour',
  accruedMicros: 'accrued_micros',
  display: 'display',
  billableSeconds: 'billable_seconds',
  accruing: 'accruing',
  unboundedIntervals: 'unbounded_intervals',
  per: 'per',
  quotedAt: 'quoted_at',
} as const;

/**
 * The JSON-Pointer paths this SDK recognizes in `STATE_DELTA` operations.
 *
 * `docs/API_AGUI.md` §3, "STATE_DELTA paths", is the complete emitted path,
 * operation, value-shape, and trigger contract. `/usage/cost` is the one
 * explicitly unwritten local placeholder, recorded at its declaration below.
 */
export const PATHS = {
  /** The session lifecycle reported by Layer 2. */
  status: '/status',
  /** The latest named step in provisioning a runtime. */
  progress: '/progress',
  /** A session's live resource readings. Replaced whole; never accumulated. */
  resources: '/resources',
  /** An open memory-pressure episode, or null when there is none. */
  memoryPressure: '/memoryPressure',
  /** Tokens the turn consumed reading. */
  inputTokens: '/usage/inputTokens',
  /** Tokens the turn produced. */
  outputTokens: '/usage/outputTokens',
  /** How full the context is, as the runtime reports it. */
  contextUsed: '/usage/contextUsed',
  /** The size of the context the reading is drawn from, where the runtime says. */
  contextWindow: '/usage/contextWindow',
  /**
   * What the session has spent.
   *
   * Declared and unwritten: no interface emits this today. Kept because the
   * surface that reads it draws nothing when it is absent, and because the
   * absence is the fact worth recording — a reader wondering why no cost ever
   * appears should find the answer here rather than in a git log. Verified
   * against the service on 2026-08-19 by listing every path it patches.
   */
  cost: '/usage/cost',
  /** The permission mode the agent is running under. */
  mode: '/mode',
  /** One pending approval, keyed by its RFC 6901-escaped request id. */
  pendingApproval: '/pendingApprovals/{requestId}',
  /** What the agent intends to do, as steps. */
  plan: '/plan',
  /** The settings this session may change, in the kernel's own words. */
  configOptions: '/configOptions',
  /**
   * Where the agent is waiting for its owner to sign in.
   *
   * Only ever written for a session spending its owner's own account with the
   * model vendor. It carries an address and a short-lived code — never a
   * credential, which is written by the agent inside its own sandbox and never
   * reaches this service.
   */
  signIn: '/signIn',
  /**
   * The actions the owner may invoke right now.
   *
   * Written by both the durable projection and cursorless live lane. Each
   * announcement replaces the list whole, so both lanes converge on one path.
   */
  availableCommands: '/availableCommands',
  /** Where a plane change is appended. */
  planeChanges: '/planeChanges/-',
  /**
   * Where a lost runtime is appended.
   *
   * Appended rather than replaced: a session can lose more than one runtime,
   * and each loss is a separate thing that happened. `noticedAt` is when the
   * service established it, which is up to one reconciliation sweep after the
   * runtime stopped — never drawn as the moment of death.
   */
  runtimeLosses: '/runtimeLosses/-',
  /** Where a stretch of lost conversation is appended. */
  gaps: '/gaps/-',
  /** Where a conversation being summarised to make room is appended. */
  contextCompactions: '/contextCompactions/-',
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
export const LIVE_MARKER = 'cma:live';

/**
 * How a client tells one stamped frame from another.
 *
 * Current projections stamp only their final frame. The kind remains part of
 * this key for compatibility with control planes that predate final-frame
 * acknowledgement and stamp every sibling with the same sequence. No
 * projection emits the same kind twice for one event, so the pair is exact.
 */
export function frameKey(seq: number, type: string): string {
  return `${String(seq)}:${type}`;
}
