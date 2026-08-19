import { ApiError } from '../api/client';
import { CREDENTIALS, EFFORTS, PERMISSIONS } from '../sessions/options';
import type { CredentialMode, PermissionMode, ReasoningEffort } from '../sessions/options';
import { DEFAULT_AGENT, PLANE_BEFORE_PLANES_EXISTED } from './types';
import type { SessionPlane } from '../sessions/types';
import type {
  AgentConfiguration,
  NyxIdAccess,
  PlatformService,
  ServiceCatalogue,
  CatalogSkill,
  EnvironmentProfile,
  EnvironmentProfileStore,
  MandatoryEntries,
  ProfileDraft,
  SkillCatalogue,
} from './types';

/**
 * Profiles as the control plane keeps them.
 *
 * The implementation of the port that is actually used. The in-memory one
 * beside it exists so components can be tested without a server, and the two
 * are interchangeable precisely because neither is reachable from a component
 * except through the port.
 *
 * Every failure arrives as an Error whose message is fit to show someone: the
 * control plane answers with a problem document, and the field a person needs
 * is the detail rather than the status.
 */

/** What the control plane sends for a profile. */
interface ProfilePayload {
  readonly id?: unknown;
  readonly name?: unknown;
  readonly variables?: unknown;
  readonly packages?: unknown;
  readonly skills?: unknown;
  readonly agent?: unknown;
  readonly nyxid?: unknown;
  readonly mandatory?: unknown;
  readonly created_at?: unknown;
  readonly updated_at?: unknown;
}

const BASE = '/api/v1/environment-profiles';

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      // The session cookie is the credential; without this every call reads as
      // anonymous.
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
      if (typeof body.detail === 'string' && body.detail !== '') {
        detail = body.detail;
      } else if (typeof body.title === 'string' && body.title !== '') {
        detail = body.title;
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

function readMandatory(raw: unknown): MandatoryEntries {
  const m = raw as { packages?: unknown; skills?: unknown } | undefined;
  return {
    packages: Array.isArray(m?.packages) ? m.packages.map(String) : [],
    skills: Array.isArray(m?.skills) ? m.skills.map(String) : [],
  };
}

/**
 * Read a profile off the wire.
 *
 * Rejects a payload with no identifier rather than rendering one that cannot
 * be edited or deleted afterwards.
 */
export function toProfile(raw: unknown): EnvironmentProfile {
  const p = raw as ProfilePayload;
  if (typeof p?.id !== 'string' || p.id === '') {
    throw new ApiError(0, 'a profile arrived with no identifier');
  }
  const variables = Array.isArray(p.variables) ? p.variables : [];
  const packages = Array.isArray(p.packages) ? p.packages : [];
  return {
    id: p.id,
    name: typeof p.name === 'string' ? p.name : '',
    variables: variables.map((v) => {
      const entry = v as { key?: unknown; value?: unknown };
      return { key: String(entry.key ?? ''), value: String(entry.value ?? '') };
    }),
    packages: packages.map((s) => {
      const entry = s as { name?: unknown; version?: unknown };
      return {
        name: String(entry.name ?? ''),
        version: typeof entry.version === 'string' ? entry.version : undefined,
      };
    }),
    agent: readAgent(p.agent),
    nyxid: readNyxId(p.nyxid),
    mandatory: readMandatory(p.mandatory),
    createdAt: typeof p.created_at === 'string' ? p.created_at : '',
    updatedAt: typeof p.updated_at === 'string' ? p.updated_at : '',
  };
}

/**
 * The agent half of a profile as the wire carries it.
 *
 * Every field falls back to what the console considers ordinary rather than to
 * nothing: a form cannot preselect a value it was not sent, and a control
 * rendered blank would read as "no answer" when the server has one.
 */
function readAgent(raw: unknown): AgentConfiguration {
  const a = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    kernel: typeof a.kernel === 'string' ? a.kernel : DEFAULT_AGENT.kernel,
    // An unstated or unrecognised plane is what it always was, not what a
    // new profile would start as — see PLANE_BEFORE_PLANES_EXISTED.
    mode: isPlane(a.mode) ? a.mode : PLANE_BEFORE_PLANES_EXISTED,
    model: typeof a.model === 'string' ? a.model : undefined,
    reasoningEffort: isEffort(a.reasoning_effort)
      ? a.reasoning_effort
      : DEFAULT_AGENT.reasoningEffort,
    permission: isPermission(a.permission) ? a.permission : DEFAULT_AGENT.permission,
    credential: isCredential(a.credential) ? a.credential : DEFAULT_AGENT.credential,
    skills: Array.isArray(a.skills) ? a.skills.map(String) : [],
  };
}

/** The platform-access half of a profile as the wire carries it. */
function readNyxId(raw: unknown): NyxIdAccess {
  const n = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return { services: Array.isArray(n.services) ? n.services.map(String) : [] };
}

function isPlane(v: unknown): v is SessionPlane {
  return v === 'terminal' || v === 'conversation';
}

function isEffort(v: unknown): v is ReasoningEffort {
  return EFFORTS.some((e) => e.id === v);
}

function isPermission(v: unknown): v is PermissionMode {
  return PERMISSIONS.some((e) => e.id === v);
}

function isCredential(v: unknown): v is CredentialMode {
  return CREDENTIALS.some((e) => e.id === v);
}

function body(draft: ProfileDraft): string {
  return JSON.stringify({
    name: draft.name,
    variables: draft.variables.map((v) => ({ key: v.key, value: v.value })),
    packages: draft.packages.map((p) => ({ name: p.name, version: p.version })),
    agent: {
      kernel: draft.agent.kernel,
      mode: draft.agent.mode,
      model: draft.agent.model,
      reasoning_effort: draft.agent.reasoningEffort,
      permission: draft.agent.permission,
      credential: draft.agent.credential,
      skills: [...draft.agent.skills],
    },
    nyxid: { services: [...draft.nyxid.services] },
  });
}

export function createRestProfileStore(): EnvironmentProfileStore {
  return {
    async list(): Promise<readonly EnvironmentProfile[]> {
      const payload = await call(BASE);
      const items = (payload as { profiles?: unknown })?.profiles ?? payload;
      if (!Array.isArray(items)) {
        throw new ApiError(0, 'the profile list was not a list');
      }
      return items.map(toProfile);
    },

    async create(draft: ProfileDraft): Promise<EnvironmentProfile> {
      return toProfile(await call(BASE, { method: 'POST', body: body(draft) }));
    },

    async update(id: string, draft: ProfileDraft): Promise<EnvironmentProfile> {
      return toProfile(
        await call(`${BASE}/${encodeURIComponent(id)}`, { method: 'PUT', body: body(draft) }),
      );
    },

    async remove(id: string): Promise<void> {
      await call(`${BASE}/${encodeURIComponent(id)}`, { method: 'DELETE' });
    },

    async services(): Promise<ServiceCatalogue> {
      const payload = (await call(`${BASE}/services`)) as {
        services?: unknown;
        mandatory?: unknown;
      };
      const services: PlatformService[] = Array.isArray(payload?.services)
        ? payload.services.map((s) => {
            const entry = s as { id?: unknown; name?: unknown; description?: unknown };
            const id = String(entry.id ?? '');
            return {
              id,
              // Falls back to the identifier rather than rendering blank: a row
              // with no label is one nobody can decide about.
              name: typeof entry.name === 'string' && entry.name !== '' ? entry.name : id,
              description: typeof entry.description === 'string' ? entry.description : undefined,
            };
          })
        : [];
      return {
        services,
        mandatory: Array.isArray(payload?.mandatory) ? payload.mandatory.map(String) : [],
      };
    },

    async catalogue(): Promise<SkillCatalogue> {
      const payload = (await call(`${BASE}/skills`)) as {
        skills?: unknown;
        mandatory?: unknown;
      };
      const skills: CatalogSkill[] = Array.isArray(payload?.skills)
        ? payload.skills.map((s) => {
            const entry = s as {
              id?: unknown;
              name?: unknown;
              description?: unknown;
              access?: unknown;
            };
            return {
              id: String(entry.id ?? ''),
              name: String(entry.name ?? ''),
              description: typeof entry.description === 'string' ? entry.description : '',
              access: typeof entry.access === 'string' ? entry.access : undefined,
            };
          })
        : [];
      return { skills, mandatory: readMandatory(payload?.mandatory) };
    },
  };
}
