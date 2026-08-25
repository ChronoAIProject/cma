import {
  ApiError,
  ProtocolError,
  request,
  requestPages,
  resolveConfig,
  type RequestOptions,
} from '@chronoai/cma-sdk';
import { PlatformSettingsConflict } from './types';
import type {
  ActivityEntry,
  ActivityQuery,
  AdminSession,
  AssumedSessionSize,
  CapacityBounds,
  DeploymentCapacity,
  DetailPair,
  OperatorStore,
  Person,
  PlatformSettings,
  ProviderReport,
  RuntimeEntry,
  RuntimeInventory,
  SessionBriefing,
  StoredPlatformSettings,
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
const config = resolveConfig();

async function call(path: string, options: RequestOptions = {}): Promise<unknown> {
  try {
    return await request(config, path, options);
  } catch (cause) {
    if (cause instanceof ApiError && cause.code === 'settings_conflict') {
      throw new PlatformSettingsConflict(cause.message);
    }
    throw cause;
  }
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
    throw new ProtocolError(`the ${what} was not a list`);
  }
  return payload;
}

/** An object-shaped payload, or a boundary failure fit to show an operator. */
function object(payload: unknown, what: string): Record<string, unknown> {
  if (typeof payload !== 'object' || payload === null || Array.isArray(payload)) {
    throw new ProtocolError(`the ${what} was not an object`);
  }
  return payload as Record<string, unknown>;
}

/** A required string that may deliberately be empty. */
function prose(value: unknown, what: string): string {
  if (typeof value !== 'string') {
    throw new ProtocolError(`the ${what} was not text`);
  }
  return value;
}

/** An ordered set of editable prose blocks. */
function proseBlocks(value: unknown, what: string): readonly string[] {
  if (!Array.isArray(value) || !value.every((entry) => typeof entry === 'string')) {
    throw new ProtocolError(`the ${what} was not a list of text blocks`);
  }
  return [...value];
}

/** A positive integer setting, never rounded or silently repaired. */
function positiveInteger(value: unknown, what: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value <= 0) {
    throw new ProtocolError(`the ${what} was not a positive integer`);
  }
  return value;
}

/** A declared aggregate may be zero, but it must survive JSON exactly. */
function nonNegativeInteger(value: unknown, what: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new ProtocolError(`the ${what} was not a non-negative safe integer`);
  }
  return value;
}

/** Read editable briefing prose off the wire. */
export function toSessionBriefing(raw: unknown): SessionBriefing {
  const briefing = object(raw, 'platform briefing');
  return {
    before: proseBlocks(briefing.before, 'briefing preamble'),
    reachHeading: prose(briefing.reach_heading, 'granted-services heading'),
    reachLead: prose(briefing.reach_lead, 'granted-services introduction'),
    reachClose: prose(briefing.reach_close, 'granted-services closing prose'),
    reachNone: prose(briefing.reach_none, 'no-services prose'),
    after: proseBlocks(briefing.after, 'briefing closing blocks'),
  };
}

/** Read all four inclusive size bounds off the wire. */
export function toCapacityBounds(raw: unknown): CapacityBounds {
  const bounds = object(raw, 'platform size bounds');
  return {
    minCpuMillis: positiveInteger(bounds.min_cpu_millis, 'minimum CPU bound'),
    maxCpuMillis: positiveInteger(bounds.max_cpu_millis, 'maximum CPU bound'),
    minMemoryMib: positiveInteger(bounds.min_memory_mib, 'minimum memory bound'),
    maxMemoryMib: positiveInteger(bounds.max_memory_mib, 'maximum memory bound'),
  };
}

function toAssumedSessionSize(raw: unknown): AssumedSessionSize {
  const size = object(raw, 'assumed session size');
  return {
    cpuMillis: positiveInteger(size.cpu_millis, 'assumed session CPU'),
    memoryMib: positiveInteger(size.memory_mib, 'assumed session memory'),
  };
}

/** Read the three nullable deployment-ledger declarations off the wire. */
export function toDeploymentCapacity(raw: unknown): DeploymentCapacity {
  // The field was added compatibly to stored settings. During a rolling
  // update an older control plane can still omit it, which means the same
  // thing as the core's default: no dimensions declared.
  if (raw === undefined) {
    return { cpuMillis: null, memoryMib: null, assumedSessionSize: null };
  }
  const capacity = object(raw, 'deployment capacity');
  return {
    cpuMillis:
      capacity.cpu_millis === null
        ? null
        : nonNegativeInteger(capacity.cpu_millis, 'deployment CPU capacity'),
    memoryMib:
      capacity.memory_mib === null
        ? null
        : nonNegativeInteger(capacity.memory_mib, 'deployment memory capacity'),
    assumedSessionSize:
      capacity.assumed_session_size === null
        ? null
        : toAssumedSessionSize(capacity.assumed_session_size),
  };
}

/** Read one complete settings object off the wire. */
export function toPlatformSettings(raw: unknown): PlatformSettings {
  const settings = object(raw, 'platform settings');
  return {
    briefing: toSessionBriefing(settings.briefing),
    deploymentCapacity: toDeploymentCapacity(settings.deployment_capacity),
    sizeBounds: toCapacityBounds(settings.size_bounds),
    liveSessions:
      settings.live_sessions === null
        ? null
        : positiveInteger(settings.live_sessions, 'live-session limit'),
  };
}

/** Read a revisioned settings response off the wire. */
export function toStoredPlatformSettings(raw: unknown): StoredPlatformSettings {
  const stored = object(raw, 'platform settings response');
  if (
    typeof stored.revision !== 'number' ||
    !Number.isSafeInteger(stored.revision) ||
    stored.revision < 0
  ) {
    throw new ProtocolError('the platform settings revision was not a non-negative integer');
  }
  return {
    revision: stored.revision,
    settings: toPlatformSettings(stored.settings),
    previous: stored.previous === undefined ? undefined : toPlatformSettings(stored.previous),
  };
}

/** Render the frontend model in the API's names, including every briefing field. */
function settingsRequest(settings: PlatformSettings): Record<string, unknown> {
  return {
    briefing: {
      before: settings.briefing.before,
      reach_heading: settings.briefing.reachHeading,
      reach_lead: settings.briefing.reachLead,
      reach_close: settings.briefing.reachClose,
      reach_none: settings.briefing.reachNone,
      after: settings.briefing.after,
    },
    deployment_capacity: {
      cpu_millis: settings.deploymentCapacity.cpuMillis,
      memory_mib: settings.deploymentCapacity.memoryMib,
      assumed_session_size:
        settings.deploymentCapacity.assumedSessionSize === null
          ? null
          : {
              cpu_millis: settings.deploymentCapacity.assumedSessionSize.cpuMillis,
              memory_mib: settings.deploymentCapacity.assumedSessionSize.memoryMib,
            },
    },
    size_bounds: {
      min_cpu_millis: settings.sizeBounds.minCpuMillis,
      max_cpu_millis: settings.sizeBounds.maxCpuMillis,
      min_memory_mib: settings.sizeBounds.minMemoryMib,
      max_memory_mib: settings.sizeBounds.maxMemoryMib,
    },
    live_sessions: settings.liveSessions,
  };
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
    throw new ProtocolError('somebody arrived in the directory with no identifier');
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
    throw new ProtocolError('an entry arrived with no identifier');
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
    throw new ProtocolError('a session arrived with no identifier');
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
      return requestPages(config, `${BASE}/sessions`, (payload) =>
        list(payload, 'session list').map(toAdminSession),
      );
    },

    async stopSession(id: string): Promise<void> {
      await call(`${BASE}/sessions/${encodeURIComponent(id)}/stop`, { method: 'POST' });
    },

    async terminateSession(id: string): Promise<void> {
      await call(`${BASE}/sessions/${encodeURIComponent(id)}/terminate`, { method: 'POST' });
    },

    async readPlatformSettings(): Promise<StoredPlatformSettings> {
      return toStoredPlatformSettings(await call(`${BASE}/settings`));
    },

    async changePlatformSettings(
      expectedRevision: number,
      settings: PlatformSettings,
    ): Promise<StoredPlatformSettings> {
      return toStoredPlatformSettings(
        await call(`${BASE}/settings`, {
          method: 'PATCH',
          body: {
            expected_revision: expectedRevision,
            ...settingsRequest(settings),
          },
        }),
      );
    },

    async resetPlatformBriefing(expectedRevision: number): Promise<StoredPlatformSettings> {
      return toStoredPlatformSettings(
        await call(`${BASE}/settings/reset`, {
          method: 'POST',
          body: { expected_revision: expectedRevision },
        }),
      );
    },
  };
}
