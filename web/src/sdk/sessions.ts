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

import { toCapabilities } from '../agui';
import { headersFor, urlOf, type ResolvedConfig } from './config';
import { ApiError } from './errors';
import { request } from './http';
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
  SessionService,
  SessionSkill,
} from './types';

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
  created_at?: unknown;
  stop_reason?: unknown;
  runtime_version?: unknown;
  rotations?: unknown;
  rotated_at?: unknown;
  creator?: unknown;
  reach?: unknown;
  conforms_to?: unknown;
  agent?: unknown;
}

/**
 * Convert a payload into the model a consumer reads.
 *
 * An unknown lifecycle is rejected rather than coerced: reporting a session in
 * a state the model does not have would misstate what the control plane said.
 */
export function toSession(raw: unknown): Session {
  const p = raw as SessionPayload;
  if (typeof p?.id !== 'string' || p.id === '') {
    throw new ApiError(0, 'session payload carried no id');
  }
  if (typeof p.status !== 'string' || !LIFECYCLES.includes(p.status as SessionLifecycle)) {
    throw new ApiError(0, `session ${p.id} has an unrecognised status`);
  }
  if (typeof p.kernel !== 'string' || p.kernel === '') {
    throw new ApiError(0, `session ${p.id} names no kernel`);
  }
  // The wire carries RFC 3339; consumers count age in milliseconds. An
  // unparseable date falls back to now rather than propagating NaN.
  const parsed = typeof p.created_at === 'string' ? Date.parse(p.created_at) : Number.NaN;
  return {
    id: p.id,
    kernel: p.kernel as Kernel,
    lifecycle: p.status as SessionLifecycle,
    createdAt: Number.isNaN(parsed) ? Date.now() : parsed,
    // Narrowed to the one distinction a consumer acts on. Carrying the control
    // plane's full vocabulary here would invite reasoning about reasons
    // nothing has behaviour for.
    stopReason: p.stop_reason === 'error' ? 'error' : p.stop_reason ? 'other' : undefined,
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
    // Absent and empty stay different facts. An entry with no usable
    // identifier is dropped rather than carried blank, exactly as in `reach`.
    skills: Array.isArray(a.skills) ? a.skills.flatMap(skill) : undefined,
  };
}

/** One granted skill, or nothing where the entry carried no identifier. */
function skill(raw: unknown): SessionSkill[] {
  const entry = raw as { id?: unknown; name?: unknown; description?: unknown } | null;
  if (typeof entry?.id !== 'string' || entry.id === '') {
    return [];
  }
  return [
    {
      id: entry.id,
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
export async function listSessions(config: ResolvedConfig): Promise<readonly Session[]> {
  const body = await request(config, '/api/v1/sessions');
  const items = (body as { sessions?: unknown })?.sessions ?? body;
  if (!Array.isArray(items)) {
    throw new ApiError(0, 'session list was not a list');
  }
  return items.map(toSession);
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
export async function readSession(config: ResolvedConfig, id: string): Promise<Session> {
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}`);
  return toSession(body);
}

export async function listKernels(config: ResolvedConfig): Promise<readonly KernelOffer[]> {
  const body = await request(config, '/api/v1/kernels');
  const items = (body as { kernels?: unknown })?.kernels;
  if (!Array.isArray(items)) {
    throw new ApiError(0, 'the kernel list was not a list');
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
    throw new ApiError(0, 'a runtime arrived with no identifier');
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
  readonly cols: number;
  /** Terminal height, in rows. */
  readonly rows: number;
  /**
   * The bundle describing the agent — which one, which model, how freely it
   * acts, what it carries.
   *
   * Omitting it is meaningful: absent means "this deployment's answer for an
   * unqualified session", which is not the same as naming a bundle.
   */
  readonly environmentProfile?: string | null;
}

/**
 * Provision a session.
 *
 * The profile is the whole description of the session, so nothing else about
 * the agent is sent.
 */
export async function createSession(
  config: ResolvedConfig,
  options: CreateSession,
): Promise<Session> {
  const body = await request(config, '/api/v1/sessions', {
    method: 'POST',
    body: {
      cols: options.cols,
      rows: options.rows,
      // Omitted rather than sent as null when nothing was chosen: absent means
      // "this deployment's answer", which is not the same as a caller naming a
      // bundle that does not exist.
      environment_profile: options.environmentProfile ?? undefined,
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
): Promise<Session> {
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/plane`, {
    method: 'POST',
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
export async function terminateSession(config: ResolvedConfig, id: string): Promise<void> {
  await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/terminate`, { method: 'POST' });
}

/**
 * Release a session's sandbox, keeping the session.
 *
 * The reversible half of ending something: nothing runs and nothing is paid
 * for, while the history stays readable and the session can be picked up again
 * with `reviveSession`.
 */
export async function stopSession(config: ResolvedConfig, id: string): Promise<void> {
  await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/stop`, { method: 'POST' });
}

/** Give a stopped session a sandbox again, under the same id. */
export async function reviveSession(config: ResolvedConfig, id: string): Promise<Session> {
  const body = await request(config, `/api/v1/sessions/${encodeURIComponent(id)}/revive`, {
    method: 'POST',
  });
  return toSession(body);
}

/** Remove a session and its history permanently. */
export async function deleteSession(config: ResolvedConfig, id: string): Promise<void> {
  await request(config, `/api/v1/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/**
 * Everything a session's terminal has shown.
 *
 * How a session with no sandbox is read. Bytes rather than text, because it is
 * an escape-sequence stream that only a terminal knows how to interpret —
 * decoding it here would corrupt exactly the sequences that carry the meaning.
 */
export async function fetchTranscript(config: ResolvedConfig, id: string): Promise<Uint8Array> {
  const response = await config.fetch(
    urlOf(config, `/api/v1/sessions/${encodeURIComponent(id)}/transcript`),
    {
      headers: await headersFor(config),
      credentials: config.cookies ? 'include' : 'same-origin',
    },
  );
  if (!response.ok) {
    throw new ApiError(response.status, 'could not read the session transcript');
  }
  return new Uint8Array(await response.arrayBuffer());
}
