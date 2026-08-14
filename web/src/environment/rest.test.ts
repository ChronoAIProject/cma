import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRestProfileStore, toProfile } from './rest';
import { DEFAULT_AGENT } from './types';

/**
 * The profile store that talks to the control plane.
 *
 * What these pin is the boundary: a payload is read into the model without
 * inventing anything, a failure arrives as a message fit to show someone, and
 * the request carries the session cookie. Everything above this consumes the
 * port and cannot tell which implementation it has.
 */

function respond(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
    arrayBuffer: () => Promise.resolve(new ArrayBuffer(0)),
  } as unknown as Response;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('reading a profile off the wire', () => {
  it('keeps what the control plane said', () => {
    const profile = toProfile({
      id: 'envp_1',
      name: 'build tools',
      variables: [{ key: 'BUILD_MODE', value: 'release' }],
      packages: [{ name: 'ripgrep', version: '14' }],
      agent: { ...DEFAULT_AGENT, skills: ['nyxid'] },
      nyxid: { services: [] },
      mandatory: { packages: ['nyxid'], skills: ['ornn-agent-manual-cli'] },
      created_at: '2026-08-14T00:00:00Z',
      updated_at: '2026-08-14T01:00:00Z',
    });
    expect(profile.name).toBe('build tools');
    expect(profile.variables[0]).toEqual({ key: 'BUILD_MODE', value: 'release' });
    expect(profile.packages[0]).toEqual({ name: 'ripgrep', version: '14' });
    expect(profile.mandatory.skills).toEqual(['ornn-agent-manual-cli']);
  });

  it('refuses a profile with no identifier', () => {
    // Rendering one would produce a row nobody can edit or delete, which reads
    // as the interface being broken rather than the payload.
    expect(() => toProfile({ name: 'nameless' })).toThrow(/identifier/i);
  });

  it('treats missing collections as empty rather than failing', () => {
    // A profile with nothing in it is a legitimate profile, and a control plane
    // that omits an empty array must not take the editor down.
    const profile = toProfile({ id: 'envp_2', name: 'bare' });
    expect(profile.variables).toEqual([]);
    expect(profile.packages).toEqual([]);
    expect(profile.agent.skills).toEqual([]);
    expect(profile.mandatory.packages).toEqual([]);
  });

  it('leaves an unstated package version absent', () => {
    // Absent means "whatever the runtime provides", which is not the same as
    // an empty version anyone could pin against.
    const profile = toProfile({ id: 'envp_3', name: 'x', packages: [{ name: 'ripgrep' }] });
    expect(profile.packages[0]?.version).toBeUndefined();
  });
});

describe('talking to the control plane', () => {
  it('sends the session cookie', async () => {
    // Without this every call reads as anonymous and the list comes back empty
    // rather than refused, which is the confusing failure.
    const fetchMock = vi.fn().mockResolvedValue(respond({ profiles: [] }));
    vi.stubGlobal('fetch', fetchMock);

    await createRestProfileStore().list();

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/environment-profiles',
      expect.objectContaining({ credentials: 'include' }),
    );
  });

  it('accepts a bare list as well as a wrapped one', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(respond([{ id: 'envp_1', name: 'a' }])));
    const profiles = await createRestProfileStore().list();
    expect(profiles).toHaveLength(1);
  });

  it('reports the control plane’s own explanation', async () => {
    // The detail names what is wrong; the status only says that something is.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(respond({ detail: 'BUILD MODE is not a usable name' }, 422)),
    );
    await expect(
      createRestProfileStore().create({
        name: 'x',
        variables: [],
        packages: [],
        agent: DEFAULT_AGENT,
        nyxid: { services: [] },
      }),
    ).rejects.toThrow(/not a usable name/);
  });

  it('reports an unreachable control plane rather than hanging', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    await expect(createRestProfileStore().list()).rejects.toThrow(/cannot reach/i);
  });

  it('replaces a profile through its own address', async () => {
    const fetchMock = vi.fn().mockResolvedValue(respond({ id: 'envp_1', name: 'renamed' }));
    vi.stubGlobal('fetch', fetchMock);

    await createRestProfileStore().update('envp 1', {
      name: 'renamed',
      variables: [],
      packages: [],
      agent: { ...DEFAULT_AGENT, skills: [] },
      nyxid: { services: [] },
    });

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    // Encoded, because an identifier is data and a path is not a place to
    // concatenate data into unescaped.
    expect(url).toBe('/api/v1/environment-profiles/envp%201');
    expect(init.method).toBe('PUT');
  });

  it('removes a profile without expecting a body back', async () => {
    // The control plane answers 204, and reading a body that is not there
    // would turn a success into an error.
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(respond(null, 204)));
    await expect(createRestProfileStore().remove('envp_1')).resolves.toBeUndefined();
  });

  it('reads the catalogue with what the platform mandates', async () => {
    // The mandated entries ride with the catalogue so someone authoring their
    // first profile is told what it will carry before they save it.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        respond({
          skills: [{ id: 'sk_1', name: 'nyxid', description: 'identity', access: 'public' }],
          mandatory: { packages: ['nyxid'], skills: ['ornn-agent-manual-cli'] },
        }),
      ),
    );

    const catalogue = await createRestProfileStore().catalogue();
    expect(catalogue.skills[0]?.name).toBe('nyxid');
    expect(catalogue.mandatory.skills).toEqual(['ornn-agent-manual-cli']);
  });
});
