import { ApiError } from '../api/client';
import type {
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
    skills: Array.isArray(p.skills) ? p.skills.map(String) : [],
    mandatory: readMandatory(p.mandatory),
    createdAt: typeof p.created_at === 'string' ? p.created_at : '',
    updatedAt: typeof p.updated_at === 'string' ? p.updated_at : '',
  };
}

function body(draft: ProfileDraft): string {
  return JSON.stringify({
    name: draft.name,
    variables: draft.variables.map((v) => ({ key: v.key, value: v.value })),
    packages: draft.packages.map((p) => ({ name: p.name, version: p.version })),
    skills: [...draft.skills],
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
