/**
 * Sessions: reading them, starting them, and ending them.
 *
 * The REST projection — `docs/API_REST.md`, priority 2 of the five interfaces
 * in `CLAUDE.md` §3. Every function here takes the config as its first
 * argument rather than reading a module-level one, so a process can hold two
 * clients at once: a test against a local deployment and one against staging,
 * or a backend acting for two different subjects. A module-level singleton
 * would make that impossible and is the usual reason an SDK has to be forked.
 *
 * Responses are validated, never trusted. This is a trust boundary: a payload
 * coerced into a type it does not have becomes `undefined` somewhere far from
 * the request that produced it, and the report that comes back is about the
 * wrong thing entirely.
 */

import { toCapabilities } from './agui/index.js';
import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, requestBytes, requestVoid, type OperationOptions } from './http.js';
import { PRICING_WIRE } from './wire.js';
import type {
  CredentialMode,
  Kernel,
  KernelOffer,
  PermissionMode,
  ReasoningEffort,
  Session,
  SessionAgent,
  SessionCreator,
  SessionLifecycle,
  SessionPlane,
  StopReason,
  SessionService,
  SessionSize,
  SessionSkill,
} from './types.js';

/*
 * The wire vocabularies this client can draw, kept beside the parser that
 * checks against them. Pinned on the other side by `option_wire_names_are_what
 * _clients_read` in `layer2-core`, because a rename there is silent here.
 */
const PERMISSION_MODES: readonly PermissionMode[] = ['guarded', 'autonomous', 'unrestricted'];
const REASONING_EFFORTS: readonly ReasoningEffort[] = [
  'minimal',
  'low',
  'medium',
  'high',
  'x_high',
];
const CREDENTIAL_MODES: readonly CredentialMode[] = ['delegated', 'own'];

const LIFECYCLES: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
];

/** Exact `StopReason` spellings serialized by layer2-core. */
export const STOP_REASONS = [
  'end_turn',
  'user_terminated',
  'idle_timeout',
  'lifetime_cap',
  'requires_action',
  'revoked',
  'budget_reached',
  'error',
] as const satisfies readonly Exclude<StopReason, 'other'>[];

// Deliberately no allowlist of kernel identifiers. This build does not decide
// which runtimes a deployment offers, and a session naming one it has not
// heard of is still that owner's session: it is carried under its own
// identifier, never refused. Refusing it here failed the entire list read, so
// one session on an unfamiliar runtime hid every other.

/** Shape the control plane sends for a session. */
interface SessionPayload {
  id?: unknown;
  kernel?: unknown;
  status?: unknown;
  recovery_state?: unknown;
  created_at?: unknown;
  stop_reason?: unknown;
  runtime_version?: unknown;
  rotations?: unknown;
  rotated_at?: unknown;
  creator?: unknown;
  reach?: unknown;
  conforms_to?: unknown;
  agent?: unknown;
  last_seq?: unknown;
  cols?: unknown;
  rows?: unknown;
}

/**
 * Convert a payload into the model a consumer reads.
 *
 * A row with no id or no string status cannot be attributed and fails loudly.
 * A string status from a newer server is still the owner's session, so it is
 * carried as `unknown` with its raw word just as an unfamiliar kernel is.
 */
export function toSession(raw: unknown): Session {
  const p = raw as SessionPayload;
  if (typeof p?.id !== 'string' || p.id === '') {
    throw new ProtocolError('session payload carried no id');
  }
  if (typeof p.status !== 'string' || p.status === '') {
    throw new ProtocolError(`session ${p.id} carried no status`);
  }
  if (typeof p.kernel !== 'string' || p.kernel === '') {
    throw new ProtocolError(`session ${p.id} names no kernel`);
  }
  // The wire carries RFC 3339; consumers count age in milliseconds. An
  // unparseable date falls back to now rather than propagating NaN.
  const parsed = typeof p.created_at === 'string' ? Date.parse(p.created_at) : Number.NaN;
  return {
    id: p.id,
    kernel: p.kernel as Kernel,
    lifecycle: LIFECYCLES.includes(p.status as SessionLifecycle)
      ? (p.status as SessionLifecycle)
      : 'unknown',
    reportedStatus: LIFECYCLES.includes(p.status as SessionLifecycle) ? undefined : p.status,
    recoveryState:
      typeof p.recovery_state === 'string' && p.recovery_state !== ''
        ? p.recovery_state
        : undefined,
    createdAt: Number.isNaN(parsed) ? Date.now() : parsed,
    stopReason: stopReason(p.stop_reason),
    lastSeq: nonNegativeInteger(p.last_seq),
    cols: positiveInteger(p.cols),
    rows: positiveInteger(p.rows),
    runtimeVersion: typeof p.runtime_version === 'string' ? p.runtime_version : undefined,
    rotations: typeof p.rotations === 'number' ? p.rotations : undefined,
    // Absent until a session has actually been moved, so an unparseable value
    // becomes absent rather than a date nobody can interpret.
    rotatedAt: rotatedAt(p.rotated_at),
    creator: creator(p.creator),
    reach: reach(p.reach),
    plane: plane(p.conforms_to),
    // Read once at the boundary, so a consumer asks what a session may do
    // rather than trying and handling a refusal.
    capabilities: toCapabilities(raw),
    agent: agent(p.agent),
  };
}

function stopReason(raw: unknown): StopReason | undefined {
  if (typeof raw !== 'string' || raw === '') {
    return undefined;
  }
  return STOP_REASONS.includes(raw as (typeof STOP_REASONS)[number])
    ? (raw as (typeof STOP_REASONS)[number])
    : 'other';
}

/**
 * How the session's agent was configured.
 *
 * Every field is validated against the vocabulary this client draws rather than
 * carried through as whatever arrived. A word the client has no rendering for
 * becomes absent, which draws as "not reported" — honest — where passing it
 * through would print a raw wire token into a panel people read.
 *
 * The whole object is absent when the control plane sent none, so a build
 * without it is told apart from a session configured with nothing.
 */
function agent(raw: unknown): SessionAgent | undefined {
  if (typeof raw !== 'object' || raw === null) {
    return undefined;
  }
  const a = raw as Record<string, unknown>;
  return {
    model: typeof a.model === 'string' && a.model !== '' ? a.model : undefined,
    permission: oneOf(a.permission, PERMISSION_MODES),
    reasoningEffort: oneOf(a.reasoning_effort, REASONING_EFFORTS),
    credential: oneOf(a.credential, CREDENTIAL_MODES),
    size: sessionSize(a.size),
    // Absent and empty stay different facts. An entry with no usable
    // identifier is dropped rather than carried blank, exactly as in `reach`.
    skills: Array.isArray(a.skills) ? a.skills.flatMap(skill) : undefined,
  };
}

function sessionSize(raw: unknown): SessionSize | undefined {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  if (typeof raw !== 'object' || Array.isArray(raw)) {
    throw new ProtocolError('session agent size was not an object');
  }
  const size = raw as Record<string, unknown>;
  return {
    cpuMillis: sessionSizeFigure(size.cpu_millis, 'cpu_millis'),
    memoryMib: sessionSizeFigure(size.memory_mib, 'memory_mib'),
  };
}

function sessionSizeFigure(raw: unknown, field: string): number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw <= 0 || raw > 4_294_967_295) {
    throw new ProtocolError(`session agent size carried an invalid ${field}`);
  }
  return raw;
}

/** One granted skill, or nothing where the entry carried no identifier. */
function skill(raw: unknown): SessionSkill[] {
  const entry = raw as {
    id?: unknown;
    name?: unknown;
    description?: unknown;
    version?: unknown;
  } | null;
  if (typeof entry?.id !== 'string' || entry.id === '') {
    return [];
  }
  return [
    {
      id: entry.id,
      version:
        typeof entry.version === 'string' && entry.version !== '' ? entry.version : undefined,
      name: typeof entry.name === 'string' && entry.name !== '' ? entry.name : undefined,
      description:
        typeof entry.description === 'string' && entry.description !== ''
          ? entry.description
          : undefined,
    },
  ];
}

/** The value when the wire said one of these words, and absent otherwise. */
function oneOf<T extends string>(raw: unknown, allowed: readonly T[]): T | undefined {
  return typeof raw === 'string' && (allowed as readonly string[]).includes(raw)
    ? (raw as T)
    : undefined;
}

/**
 * Which plane a session is on, read from what it conforms to.
 *
 * The control plane reports granted classes as URIs ending `/conf/<name>`;
 * `conversation` and `terminal` are two of them and are exclusive by
 * construction. Read from the session rather than assumed, because it is the
 * session's own answer and no interface chooses one.
 *
 * A session declaring neither reads as terminal — what every session was
 * before the conversation plane existed.
 */
function plane(raw: unknown): SessionPlane | undefined {
  const granted = toCapabilities({ conforms_to: raw });
  if (granted.conversation) {
    return 'conversation';
  }
  return granted.terminal ? 'terminal' : undefined;
}

/**
 * What this session's credential may reach, as the control plane reported it.
 *
 * `undefined` when the field is absent — an older control plane, or one that
 * does not project it — and an empty array when it is present and empty. The
 * two are different facts: "we were not told" is not "this session reaches
 * nothing", and collapsing them answers a question nobody asked.
 *
 * An entry with no usable identifier is dropped rather than carried blank. No
 * entry is filtered for credential material, because the projection carries
 * none — see `docs/API_REST.md`.
 */
function reach(raw: unknown): readonly SessionService[] | undefined {
  if (!Array.isArray(raw)) {
    return undefined;
  }
  return raw.flatMap((entry): SessionService[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const service = entry as Record<string, unknown>;
    if (typeof service.id !== 'string' || service.id === '') {
      return [];
    }
    return [
      {
        id: service.id,
        name: typeof service.name === 'string' ? service.name : undefined,
        description: typeof service.description === 'string' ? service.description : undefined,
        // Absent reads as not mandated, which is the safe direction: reporting
        // a chosen service as fixed would tell somebody they cannot remove
        // something they can.
        mandated: service.mandated === true,
      },
    ];
  });
}

/** The person who started a session, when the control plane named them. */
function creator(raw: unknown): SessionCreator | undefined {
  if (typeof raw !== 'object' || raw === null) {
    return undefined;
  }
  const c = raw as Record<string, unknown>;
  const displayName = typeof c.display_name === 'string' ? c.display_name : undefined;
  const avatarUrl = typeof c.avatar_url === 'string' ? c.avatar_url : undefined;
  // Both absent is the same as no creator at all: an object with nothing in it
  // would have a consumer draw a heading over blank space.
  return displayName === undefined && avatarUrl === undefined
    ? undefined
    : { displayName, avatarUrl };
}

/** The snapshot time a session was carried across from, if it was. */
function rotatedAt(raw: unknown): number | undefined {
  if (typeof raw !== 'string') {
    return undefined;
  }
  const parsed = Date.parse(raw);
  return Number.isNaN(parsed) ? undefined : parsed;
}

/** Sessions belonging to the authenticated subject, newest first. */
export async function listSessions(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<readonly Session[]> {
  const body = await request(config, '/api/v1/sessions', { ...options, expectedStatus: 200 });
  const items = (body as { sessions?: unknown })?.sessions ?? body;
  if (!Array.isArray(items)) {
    throw new ProtocolError('session list was not a list');
  }
  return items.map(toSession);
}

/** The authenticated subject's current live-session allowance. */
export interface SessionAllowance {
  /** Absent when the deployment has configured no limit. */
  readonly limit?: number;
  readonly live: number;
  readonly exempt: boolean;
}

/**
 * Read the authenticated subject's advisory live-session allowance.
 *
 * Invalid fields fall in the permissive direction because the create or
 * revive request remains authoritative. An older or malformed advisory
 * response must not disable the only way to start work.
 */
export async function readAllowance(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<SessionAllowance> {
  const body = (await request(config, '/api/v1/sessions/allowance', {
    ...options,
    expectedStatus: 200,
  })) as {
    limit?: unknown;
    live?: unknown;
    exempt?: unknown;
  } | null;
  const limit = positiveInteger(body?.limit);
  return {
    ...(limit === undefined ? {} : { limit }),
    live: nonNegativeInteger(body?.live) ?? 0,
    exempt: body?.exempt === true,
  };
}

function positiveInteger(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}

function nonNegativeInteger(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

/**
 * The runtimes this deployment can start a session on, and what each can be.
 *
 * Worth asking once and passing down rather than wherever a caller needs it:
 * the answer describes the deployment and does not change underneath anybody.
 */
/**
 * Read one session as the control plane currently describes it.
 *
 * Needed because a session's record is not complete when it is created. Some
 * of what a client draws is settled during provisioning — which skills were
 * actually installed, which runtime version it landed on — and a client
 * holding the object it got back from `POST /sessions` is holding the answer
 * from before any of that happened.
 */
export async function readSession(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<Session> {
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}`, {
    ...options,
    expectedStatus: 200,
  });
  return toSession(body);
}

export async function listKernels(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<readonly KernelOffer[]> {
  const body = await request(config, '/api/v1/kernels', { ...options, expectedStatus: 200 });
  const items = (body as { kernels?: unknown })?.kernels;
  if (!Array.isArray(items)) {
    throw new ProtocolError('the kernel list was not a list');
  }
  return items.map(toKernelOffer);
}

/**
 * Read one runtime's offer.
 *
 * A plane this build has no word for is dropped rather than carried, so a
 * consumer is never offered a choice that ends somewhere nobody has written. A
 * runtime left with no planes is still listed, because it exists and saying
 * otherwise is a different claim.
 */
function toKernelOffer(raw: unknown): KernelOffer {
  const k = raw as { id?: unknown; modes?: unknown };
  if (typeof k?.id !== 'string' || k.id === '') {
    throw new ProtocolError('a runtime arrived with no identifier');
  }
  const modes = Array.isArray(k.modes) ? k.modes : [];
  return { id: k.id, modes: modes.filter(isPlane) };
}

function isPlane(value: unknown): value is SessionPlane {
  return value === 'terminal' || value === 'conversation';
}

/** What a new session is asked for. */
export interface CreateSession {
  /** Terminal width, in columns. */
  readonly cols?: number;
  /** Terminal height, in rows. */
  readonly rows?: number;
  /**
   * The bundle describing the agent — which one, which model, how freely it
   * acts, what it carries.
   *
   * Omitting it is meaningful: absent means "this deployment's answer for an
   * unqualified session", which is not the same as naming a bundle.
   */
  readonly environmentProfile?: string | null;
  /** The sealed card whose displayed rate the owner accepted. */
  readonly quotedRateCard?: string;
}

/** A new session has no configured capacity price. */
export interface UnpricedQuote {
  readonly priced: false;
}

/** The exact hourly rate the service computed for a new session. */
export interface PricedQuote {
  readonly priced: true;
  readonly rateCard: string;
  readonly currency: string;
  readonly cpuMillis: number;
  readonly memoryMib: number;
  readonly microsPerHour: string;
  readonly display: string;
  readonly per: 'hour';
  readonly quotedAt: string;
}

export type PricingQuote = PricedQuote | UnpricedQuote;

/** A session created without a configured price. */
export interface UnpricedSpend {
  readonly priced: false;
  readonly accruing: false;
  readonly unboundedIntervals: number;
}

/** Reserved-capacity spend derived by the service from durable history. */
export interface PricedSpend {
  readonly priced: true;
  readonly rateCard: string;
  readonly currency: string;
  readonly microsPerHour: string;
  readonly accruedMicros: string;
  readonly display: string;
  readonly billableSeconds: number;
  readonly accruing: boolean;
  readonly unboundedIntervals: number;
}

export type SessionSpend = PricedSpend | UnpricedSpend;

/** Read the exact rate a session created from this profile would pin. */
export async function quoteSession(
  config: ResolvedConfig,
  environmentProfile?: string | null,
  options: OperationOptions = {},
): Promise<PricingQuote> {
  const query = new URLSearchParams();
  if (environmentProfile !== undefined && environmentProfile !== null) {
    query.set(PRICING_WIRE.environmentProfile, environmentProfile);
  }
  const suffix = query.size === 0 ? '' : `?${query.toString()}`;
  return toPricingQuote(
    await request(config, `${PRICING_WIRE.quotePath}${suffix}`, {
      ...options,
      expectedStatus: 200,
    }),
  );
}

/** Read a session's reserved-capacity spend, never the model vendor's cost. */
export async function readSpend(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<SessionSpend> {
  const body = await request(
    config,
    `/api/v1/sessions/${encodeURIComponent(id)}/${PRICING_WIRE.spend}`,
    { ...options, expectedStatus: 200 },
  );
  return toSessionSpend(body);
}

/** Validate a quote at the network boundary and keep monetary integers as strings. */
export function toPricingQuote(raw: unknown): PricingQuote {
  const body = objectOf(raw, 'pricing quote');
  if (body[PRICING_WIRE.priced] === false) {
    return { priced: false };
  }
  const per = textOf(body, PRICING_WIRE.per, 'pricing quote');
  if (per !== 'hour') {
    throw new ProtocolError('pricing quote used an unrecognised interval');
  }
  return {
    priced: true,
    rateCard: textOf(body, PRICING_WIRE.rateCard, 'pricing quote'),
    currency: textOf(body, PRICING_WIRE.currency, 'pricing quote'),
    cpuMillis: integerOf(body, PRICING_WIRE.cpuMillis, 'pricing quote'),
    memoryMib: integerOf(body, PRICING_WIRE.memoryMib, 'pricing quote'),
    microsPerHour: decimalOf(body, PRICING_WIRE.microsPerHour, 'pricing quote'),
    display: textOf(body, PRICING_WIRE.display, 'pricing quote'),
    per,
    quotedAt: textOf(body, PRICING_WIRE.quotedAt, 'pricing quote'),
  };
}

/** Validate a spend response without deriving or rounding any monetary value. */
export function toSessionSpend(raw: unknown): SessionSpend {
  const body = objectOf(raw, 'session spend');
  const unboundedIntervals = integerOf(body, PRICING_WIRE.unboundedIntervals, 'session spend');
  if (body[PRICING_WIRE.priced] === false) {
    return { priced: false, accruing: false, unboundedIntervals };
  }
  const accruing = body[PRICING_WIRE.accruing];
  if (body[PRICING_WIRE.priced] !== true || typeof accruing !== 'boolean') {
    throw new ProtocolError('session spend carried no pricing state');
  }
  return {
    priced: true,
    rateCard: textOf(body, PRICING_WIRE.rateCard, 'session spend'),
    currency: textOf(body, PRICING_WIRE.currency, 'session spend'),
    microsPerHour: decimalOf(body, PRICING_WIRE.microsPerHour, 'session spend'),
    accruedMicros: decimalOf(body, PRICING_WIRE.accruedMicros, 'session spend'),
    display: textOf(body, PRICING_WIRE.display, 'session spend'),
    billableSeconds: integerOf(body, PRICING_WIRE.billableSeconds, 'session spend'),
    accruing,
    unboundedIntervals,
  };
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function textOf(body: Record<string, unknown>, field: string, name: string): string {
  const value = body[field];
  if (typeof value !== 'string' || value === '') {
    throw new ProtocolError(`${name} carried no ${field}`);
  }
  return value;
}

function decimalOf(body: Record<string, unknown>, field: string, name: string): string {
  const value = body[field];
  if (typeof value !== 'string' || !/^(0|[1-9]\d*)$/.test(value)) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value;
}

function integerOf(body: Record<string, unknown>, field: string, name: string): number {
  const value = body[field];
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value;
}

/**
 * Provision a session.
 *
 * The profile is the whole description of the session, so nothing else about
 * the agent is sent.
 */
export async function createSession(
  config: ResolvedConfig,
  session: CreateSession = {},
  options: OperationOptions = {},
): Promise<Session> {
  const geometry = geometryOf(session.cols, session.rows, 'session creation');
  const body = await request(config, '/api/v1/sessions', {
    ...options,
    method: 'POST',
    expectedStatus: 201,
    body: {
      cols: geometry?.cols,
      rows: geometry?.rows,
      // Omitted rather than sent as null when nothing was chosen: absent means
      // "this deployment's answer", which is not the same as a caller naming a
      // bundle that does not exist.
      environment_profile: session.environmentProfile ?? undefined,
      [PRICING_WIRE.quotedRateCard]: session.quotedRateCard,
    },
  });
  return toSession(body);
}

/**
 * Move a session onto the other plane.
 *
 * A rotation, and answered as one: the session comes back `rotating`, which is
 * the window in which it takes no input. What it costs — the turn in flight,
 * and the pause — is worth stating before this is called, never after.
 *
 * A refusal arrives as `CapabilityRefused` naming the ability that is missing.
 * An API that names what is absent is worth nothing if the consumer reports it
 * as "something went wrong".
 */
export async function changePlane(
  config: ResolvedConfig,
  id: string,
  plane: SessionPlane,
  options: OperationOptions = {},
): Promise<Session> {
  if (!isPlane(plane)) {
    throw new ValidationError("plane must be 'terminal' or 'conversation'");
  }
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/plane`, {
    ...options,
    method: 'POST',
    expectedStatus: 200,
    body: { plane },
  });
  return toSession(body);
}

/**
 * End a session, leaving it listed.
 *
 * Distinct from removing it: a finished conversation is still worth reading,
 * and the control plane keeps it until its owner deletes it.
 */
export async function terminateSession(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, `/api/v1/sessions/${encodeURIComponent(id)}/terminate`, {
    ...options,
    method: 'POST',
    expectedStatus: 204,
  });
}

/**
 * Release a session's sandbox, keeping the session.
 *
 * The reversible half of ending something: nothing runs and nothing is paid
 * for, while the history stays readable and the session can be picked up again
 * with `reviveSession`.
 */
export async function stopSession(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, `/api/v1/sessions/${encodeURIComponent(id)}/stop`, {
    ...options,
    method: 'POST',
    expectedStatus: 204,
  });
}

/** Give a stopped session a sandbox again, under the same id. */
export async function reviveSession(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<Session> {
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/revive`, {
    ...options,
    method: 'POST',
    expectedStatus: 200,
  });
  return toSession(body);
}

/** Remove a session and its history permanently. */
export async function deleteSession(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, `/api/v1/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
    expectedStatus: 204,
  });
}

/**
 * Everything a session's terminal has shown.
 *
 * How a session with no sandbox is read. Bytes rather than text, because it is
 * an escape-sequence stream that only a terminal knows how to interpret —
 * decoding it here would corrupt exactly the sequences that carry the meaning.
 */
export async function fetchTranscript(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<Uint8Array> {
  return requestBytes(config, `/api/v1/sessions/${encodeURIComponent(id)}/transcript`, {
    ...options,
    expectedStatus: 200,
    headers: { Accept: 'application/octet-stream' },
  });
}

function geometryOf(
  cols: number | undefined,
  rows: number | undefined,
  boundary: string,
): { cols: number; rows: number } | undefined {
  if (cols === undefined && rows === undefined) {
    return undefined;
  }
  if (!wireDimension(cols) || !wireDimension(rows)) {
    throw new ValidationError(
      `${boundary} requires columns and rows between 1 and 65535 when either is supplied`,
    );
  }
  return { cols, rows };
}

function wireDimension(value: number | undefined): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 && value <= 65_535;
}
