import { ApiError } from '../api/client';
import type {
  ActivityEntry,
  ActivityQuery,
  AdminSession,
  DetailPair,
  OperatorStore,
  Person,
  ProviderReport,
  RuntimeEntry,
  RuntimeInventory,
} from './types';

/**
 * The operator surface as the control plane serves it.
 *
 * The implementation of the port that is actually used; the in-memory one
 * beside it exists so the panel can be tested without a server, and the two are
 * interchangeable precisely because no component can reach either except
 * through the port.
 *
 * Payloads are read defensively. This is a trust boundary, and a field the
 * control plane omits must produce a visible absence rather than `undefined`
 * propagating into a row that renders as blank.
 *
 * One reading rule is load-bearing: a provider is treated as observable **only**
 * when it says so and hands over a list. Anything else becomes the unobservable
 * variant, so a malformed payload degrades into "we could not tell" rather than
 * into "nothing is running".
 */

const BASE = '/api/v1/admin';

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      // The session cookie is the credential; without this every call reads as
      // anonymous and comes back refused.
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
    });
  } catch (cause) {
    throw new ApiError(0, `cannot reach the control plane: ${String(cause)}`);
  }
  if (!response.ok) {
    let detail = `request failed with ${response.status}`;
    try {
      const body = (await response.json()) as { detail?: unknown; title?: unknown };
      const text = body?.detail ?? body?.title;
      if (typeof text === 'string' && text !== '') {
        detail = text;
      }
    } catch {
      // A body that is not a problem document leaves the status as the whole
      // story, which is still better than reporting nothing.
    }
    throw new ApiError(response.status, detail);
  }
  if (response.status === 204) {
    return null;
  }
  return (await response.json()) as unknown;
}

/** A field the control plane may have omitted. */
function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

/** An RFC 3339 timestamp as an instant, or nothing if it cannot be read. */
export function instant(value: unknown): number | undefined {
  if (typeof value !== 'string') {
    return undefined;
  }
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? undefined : parsed;
}

function list(payload: unknown, what: string): readonly unknown[] {
  if (!Array.isArray(payload)) {
    throw new ApiError(0, `the ${what} was not a list`);
  }
  return payload;
}

/** Read one person off the wire. */
export function toPerson(raw: unknown): Person {
  const p = raw as {
    subject?: unknown;
    display_name?: unknown;
    email?: unknown;
    first_seen?: unknown;
    last_seen?: unknown;
  };
  if (typeof p?.subject !== 'string' || p.subject === '') {
    throw new ApiError(0, 'somebody arrived in the directory with no identifier');
  }
  return {
    subject: p.subject,
    displayName: text(p.display_name),
    email: text(p.email),
    firstSeen: instant(p.first_seen),
    lastSeen: instant(p.last_seen),
  };
}

/** The allowlisted parameters of an entry, in the order they were sent. */
function toDetail(raw: unknown): readonly DetailPair[] {
  if (typeof raw !== 'object' || raw === null) {
    return [];
  }
  return Object.entries(raw as Record<string, unknown>).map(([key, value]) => ({
    key,
    value: String(value),
  }));
}

/** Read one recorded action off the wire. */
export function toActivityEntry(raw: unknown): ActivityEntry {
  const e = raw as {
    id?: unknown;
    at?: unknown;
    actor?: unknown;
    action?: unknown;
    target?: unknown;
    outcome?: unknown;
    detail?: unknown;
  };
  if (typeof e?.id !== 'string' || e.id === '') {
    throw new ApiError(0, 'an entry arrived with no identifier');
  }
  const target = e.target as { kind?: unknown; id?: unknown } | undefined;
  return {
    id: e.id,
    at: instant(e.at),
    actor: text(e.actor),
    action: typeof e.action === 'string' ? e.action : 'unknown',
    target:
      typeof target?.id === 'string'
        ? { kind: typeof target.kind === 'string' ? target.kind : 'unknown', id: target.id }
        : undefined,
    outcome: typeof e.outcome === 'string' ? e.outcome : 'unknown',
    detail: toDetail(e.detail),
  };
}

/** Read one session off the wire. */
export function toAdminSession(raw: unknown): AdminSession {
  const s = raw as {
    id?: unknown;
    owner?: unknown;
    owner_label?: unknown;
    status?: unknown;
    recovery_state?: unknown;
    stop_reason?: unknown;
    kernel?: unknown;
    created_at?: unknown;
    runtime_version?: unknown;
  };
  if (typeof s?.id !== 'string' || s.id === '') {
    throw new ApiError(0, 'a session arrived with no identifier');
  }
  return {
    id: s.id,
    owner: typeof s.owner === 'string' ? s.owner : '',
    ownerLabel: text(s.owner_label),
    // Kept as sent rather than checked against a list the frontend knows. An
    // operator console that refused to draw a session in an unfamiliar state
    // would hide the session most worth looking at.
    status: typeof s.status === 'string' ? s.status : 'unknown',
    recoveryState: text(s.recovery_state),
    stopReason: text(s.stop_reason),
    kernel: typeof s.kernel === 'string' ? s.kernel : 'unknown',
    createdAt: instant(s.created_at),
    runtimeVersion: text(s.runtime_version),
  };
}

function toRuntimeEntry(raw: unknown): RuntimeEntry {
  const r = raw as {
    runtime?: unknown;
    session?: unknown;
    owner?: unknown;
    status?: unknown;
    unaccounted?: unknown;
  };
  return {
    runtime: typeof r?.runtime === 'string' ? r.runtime : 'unknown',
    session: text(r?.session),
    owner: text(r?.owner),
    status: text(r?.status),
    // Absent means the control plane did not say, and a runtime nobody is
    // entitled to is worth flagging when it says nothing either way.
    unaccounted: r?.unaccounted === true || text(r?.owner) === undefined,
  };
}

/**
 * Read what one provider said.
 *
 * Observable only when it both says so and hands over a list. Anything else —
 * `observable: false`, a missing array, a malformed payload — becomes the
 * unobservable variant, because the failure to avoid is drawing an idle
 * namespace on the strength of a list that was never there.
 */
export function toProviderReport(raw: unknown): ProviderReport {
  const p = raw as {
    kernel?: unknown;
    observable?: unknown;
    reason?: unknown;
    runtimes?: unknown;
  };
  const kernel = typeof p?.kernel === 'string' ? p.kernel : 'unknown';
  if (p?.observable === true && Array.isArray(p.runtimes)) {
    return { kernel, observable: true, runtimes: p.runtimes.map(toRuntimeEntry) };
  }
  return {
    kernel,
    observable: false,
    reason: text(p?.reason) ?? 'The control plane did not say why.',
  };
}

/** Read the inventory off the wire. */
export function toInventory(raw: unknown): RuntimeInventory {
  const i = raw as { running?: unknown; fully_observed?: unknown; providers?: unknown };
  const providers = Array.isArray(i?.providers) ? i.providers.map(toProviderReport) : [];
  const observed = providers.every((p) => p.observable);
  return {
    // Withheld unless every provider answered, whatever the payload claims: a
    // total that silently excludes a provider is the same lie as an empty list.
    running: observed && typeof i?.running === 'number' ? i.running : undefined,
    fullyObserved: i?.fully_observed === true && observed,
    providers,
  };
}

/** Query string for a read of the record, omitting what was not asked for. */
export function activityQuery(query: ActivityQuery): string {
  const params = new URLSearchParams();
  if (query.actor !== undefined && query.actor !== '') {
    params.set('actor', query.actor);
  }
  if (query.limit !== undefined) {
    params.set('limit', String(query.limit));
  }
  const rendered = params.toString();
  return rendered === '' ? '' : `?${rendered}`;
}

export function createRestOperatorStore(): OperatorStore {
  return {
    async people(): Promise<readonly Person[]> {
      return list(await call(`${BASE}/users`), 'directory').map(toPerson);
    },

    async activity(query: ActivityQuery): Promise<readonly ActivityEntry[]> {
      const payload = await call(`${BASE}/activity${activityQuery(query)}`);
      return list(payload, 'activity log').map(toActivityEntry);
    },

    async runtimes(): Promise<RuntimeInventory> {
      return toInventory(await call(`${BASE}/runtimes`));
    },

    async sessions(): Promise<readonly AdminSession[]> {
      return list(await call(`${BASE}/sessions`), 'session list').map(toAdminSession);
    },

    async stopSession(id: string): Promise<void> {
      await call(`${BASE}/sessions/${encodeURIComponent(id)}/stop`, { method: 'POST' });
    },

    async terminateSession(id: string): Promise<void> {
      await call(`${BASE}/sessions/${encodeURIComponent(id)}/terminate`, { method: 'POST' });
    },
  };
}
