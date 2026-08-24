import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import {
  createEnvironmentProfile,
  deleteEnvironmentProfile,
  listEnvironmentProfiles,
  listEnvironmentProfilesPage,
  preferEnvironmentProfile,
  readEnvironmentProfile,
  readServiceCatalogue,
  readSkillCatalogue,
  toEnvironmentProfile,
  updateEnvironmentProfile,
  type EnvironmentProfileDraft,
} from '../src/environment.js';
import { resolveConfig } from '../src/config.js';
import { ProtocolError, ValidationError } from '../src/errors.js';

/** @vitest-environment node */

const PROFILE = {
  id: 'envp_1',
  name: 'Production agent',
  variables: [{ key: 'REGION', value: 'ap-southeast-1' }],
  packages: [{ name: 'ripgrep', version: '14.1.1' }, { name: 'jq' }],
  agent: {
    kernel: 'codex',
    mode: 'conversation',
    model: 'gpt-5.6',
    reasoning_effort: 'x_high',
    permission: 'autonomous',
    credential: 'delegated',
    size: { cpu_millis: 1_500, memory_mib: 3_072 },
    record_tool_bodies: true,
    skills: ['nyxid', 'platform-map'],
  },
  nyxid: { services: ['github', 'slack'] },
  agent_notes: 'Keep the release trail exact.',
  mandatory: { packages: ['ca-certificates'], skills: ['nyxid'] },
  created_at: '2026-08-24T01:00:00Z',
  updated_at: '2026-08-24T02:00:00Z',
  preferred: true,
} as const;

const DRAFT: EnvironmentProfileDraft = {
  name: 'Production agent',
  variables: [{ key: 'REGION', value: 'ap-southeast-1' }],
  packages: [{ name: 'ripgrep', version: '14.1.1' }, { name: 'jq' }],
  agent: {
    kernel: 'codex',
    mode: 'conversation',
    model: 'gpt-5.6',
    reasoningEffort: 'x_high',
    permission: 'autonomous',
    credential: 'delegated',
    size: { cpuMillis: 1_500, memoryMib: 3_072 },
    recordToolBodies: true,
    skills: ['nyxid', 'platform-map'],
  },
  nyxid: { services: ['github', 'slack'] },
  agentNotes: 'Keep the release trail exact.',
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function service() {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = vi.fn((input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    const request = init ?? {};
    calls.push({ url, init: request });
    if (url.endsWith('/skills')) {
      return Promise.resolve(
        json({
          skills: [
            { id: 'platform-map', name: 'Platform map', description: 'Maps the platform.' },
            {
              id: 'private-skill',
              name: 'Private skill',
              description: 'Owned by this subject.',
              access: 'owner',
            },
          ],
          mandatory: { packages: ['ca-certificates'], skills: ['nyxid'] },
        }),
      );
    }
    if (url.endsWith('/services')) {
      return Promise.resolve(
        json({
          services: [
            { id: 'github', name: 'GitHub', description: 'Repositories and pull requests.' },
            { id: 'slack', name: 'Slack' },
          ],
          mandatory: ['model-provider'],
        }),
      );
    }
    if (request.method === 'DELETE') {
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    if (
      url.endsWith('/preferred') ||
      ((request.method ?? 'GET') === 'GET' && !url.includes('envp'))
    ) {
      return Promise.resolve(json([PROFILE]));
    }
    return Promise.resolve(json(PROFILE, request.method === 'POST' ? 201 : 200));
  });
  return {
    calls,
    config: resolveConfig({ fetch: fetch as typeof globalThis.fetch, cookies: false }),
    fetch,
  };
}

describe('environment profile responses', () => {
  it('reads every accepted agent and profile field', () => {
    expect(toEnvironmentProfile(PROFILE)).toEqual({
      id: 'envp_1',
      name: 'Production agent',
      variables: [{ key: 'REGION', value: 'ap-southeast-1' }],
      packages: [
        { name: 'ripgrep', version: '14.1.1' },
        { name: 'jq', version: undefined },
      ],
      agent: {
        kernel: 'codex',
        mode: 'conversation',
        model: 'gpt-5.6',
        reasoningEffort: 'x_high',
        permission: 'autonomous',
        credential: 'delegated',
        size: { cpuMillis: 1_500, memoryMib: 3_072 },
        recordToolBodies: true,
        skills: ['nyxid', 'platform-map'],
      },
      nyxid: { services: ['github', 'slack'] },
      agentNotes: 'Keep the release trail exact.',
      mandatory: { packages: ['ca-certificates'], skills: ['nyxid'] },
      createdAt: '2026-08-24T01:00:00Z',
      updatedAt: '2026-08-24T02:00:00Z',
      preferred: true,
    });
  });

  it('keeps the pre-#485 omission distinguishable from false', () => {
    const legacy = {
      ...PROFILE,
      agent: Object.fromEntries(
        Object.entries(PROFILE.agent).filter(([field]) => field !== 'record_tool_bodies'),
      ),
    };
    expect(toEnvironmentProfile(legacy).agent.recordToolBodies).toBeUndefined();
  });

  it('reports malformed known fields instead of filling in defaults', () => {
    expect(() =>
      toEnvironmentProfile({ ...PROFILE, agent: { ...PROFILE.agent, record_tool_bodies: 'yes' } }),
    ).toThrow(ProtocolError);
    expect(() => toEnvironmentProfile({ ...PROFILE, preferred: 'yes' })).toThrow(ProtocolError);
    expect(() => toEnvironmentProfile({ ...PROFILE, variables: 'REGION=x' })).toThrow(
      ProtocolError,
    );
  });
});

describe('the environment profile routes', () => {
  it('exposes one bounded profile page and follows its opaque target manually', async () => {
    const second = { ...PROFILE, id: 'envp_2', name: 'Older profile' };
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([PROFILE]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            link: '</api/v1/environment-profiles?limit=1&cursor=opaque-next>; rel="next"',
          },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([second]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const first = await listEnvironmentProfilesPage(config, { limit: 1, cursor: 'opaque start' });
    expect(first.items.map(({ id }) => id)).toEqual(['envp_1']);
    const next = first.next;
    expect(next).toBeDefined();
    if (next === undefined) {
      throw new Error('first page did not carry its continuation');
    }
    const last = await listEnvironmentProfilesPage(config, { next });
    expect(last.items.map(({ id }) => id)).toEqual(['envp_2']);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/environment-profiles?limit=1&cursor=opaque+start',
      '/api/v1/environment-profiles?limit=1&cursor=opaque-next',
    ]);
  });

  it('follows profile continuation links without changing the public list shape', async () => {
    const second = { ...PROFILE, id: 'envp_2', name: 'Older profile' };
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([PROFILE]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            link: '</api/v1/environment-profiles?limit=50&cursor=opaque>; rel="next"',
          },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([second]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );

    const profiles = await listEnvironmentProfiles(
      resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
    );

    expect(profiles.map(({ id }) => id)).toEqual(['envp_1', 'envp_2']);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/environment-profiles',
      '/api/v1/environment-profiles?limit=50&cursor=opaque',
    ]);
  });

  it('reads preference changes as a bounded change set rather than another list walk', async () => {
    const previous = { ...PROFILE, preferred: false };
    const selected = { ...PROFILE, id: 'envp_51', preferred: true };
    const fetch = vi.fn().mockResolvedValue(json([previous, selected]));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const changed = await preferEnvironmentProfile(config, selected.id);

    expect(changed.map(({ id, preferred }) => ({ id, preferred }))).toEqual([
      { id: 'envp_1', preferred: false },
      { id: 'envp_51', preferred: true },
    ]);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('rejects a preference response that exceeds the two-row action boundary', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        json([PROFILE, { ...PROFILE, id: 'envp_2' }, { ...PROFILE, id: 'envp_3' }]),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(preferEnvironmentProfile(config, null)).rejects.toThrow(ProtocolError);
  });

  it('covers list, read, create, replace, delete, prefer, and both catalogues', async () => {
    const { calls, config } = service();

    await expect(listEnvironmentProfiles(config)).resolves.toHaveLength(1);
    await expect(readEnvironmentProfile(config, 'envp/one')).resolves.toMatchObject({
      id: 'envp_1',
    });
    await expect(createEnvironmentProfile(config, DRAFT)).resolves.toMatchObject({ id: 'envp_1' });
    await expect(updateEnvironmentProfile(config, 'envp/one', DRAFT)).resolves.toMatchObject({
      id: 'envp_1',
    });
    await expect(deleteEnvironmentProfile(config, 'envp/one')).resolves.toBeUndefined();
    await expect(preferEnvironmentProfile(config, null)).resolves.toHaveLength(1);
    await expect(readSkillCatalogue(config)).resolves.toMatchObject({
      skills: [{ id: 'platform-map' }, { id: 'private-skill', access: 'owner' }],
      mandatory: { packages: ['ca-certificates'], skills: ['nyxid'] },
    });
    await expect(readServiceCatalogue(config)).resolves.toEqual({
      services: [
        { id: 'github', name: 'GitHub', description: 'Repositories and pull requests.' },
        { id: 'slack', name: 'Slack', description: undefined },
      ],
      mandatory: ['model-provider'],
    });

    expect(calls.map(({ url, init }) => [url, init.method ?? 'GET'])).toEqual([
      ['/api/v1/environment-profiles', 'GET'],
      ['/api/v1/environment-profiles/envp%2Fone', 'GET'],
      ['/api/v1/environment-profiles', 'POST'],
      ['/api/v1/environment-profiles/envp%2Fone', 'PUT'],
      ['/api/v1/environment-profiles/envp%2Fone', 'DELETE'],
      ['/api/v1/environment-profiles/preferred', 'PUT'],
      ['/api/v1/environment-profiles/skills', 'GET'],
      ['/api/v1/environment-profiles/services', 'GET'],
    ]);
    expect(JSON.parse(String(calls[2]?.init.body))).toEqual({
      name: 'Production agent',
      variables: [{ key: 'REGION', value: 'ap-southeast-1' }],
      packages: [{ name: 'ripgrep', version: '14.1.1' }, { name: 'jq' }],
      agent: {
        kernel: 'codex',
        mode: 'conversation',
        model: 'gpt-5.6',
        reasoning_effort: 'x_high',
        permission: 'autonomous',
        credential: 'delegated',
        size: { cpu_millis: 1_500, memory_mib: 3_072 },
        record_tool_bodies: true,
        skills: ['nyxid', 'platform-map'],
      },
      nyxid: { services: ['github', 'slack'] },
      agent_notes: 'Keep the release trail exact.',
    });
    expect(JSON.parse(String(calls[5]?.init.body))).toEqual({ profile: null });
    expect(calls.every(({ init }) => init.credentials === 'omit')).toBe(true);
  });

  it('accepts a name-only draft and lets the service apply its defaults', async () => {
    const { calls, config } = service();
    await createEnvironmentProfile(config, { name: 'Ordinary' });

    expect(JSON.parse(String(calls[0]?.init.body))).toEqual({
      name: 'Ordinary',
      variables: [],
      packages: [],
    });
  });

  it('validates identifiers and nested draft fields before sending', async () => {
    const { config, fetch } = service();
    await expect(readEnvironmentProfile(config, '')).rejects.toBeInstanceOf(ValidationError);
    await expect(
      createEnvironmentProfile(config, {
        ...DRAFT,
        agent: { ...DRAFT.agent, size: { cpuMillis: 0, memoryMib: 512 } },
      }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('is exposed through the configured client', async () => {
    const { config } = service();
    const cma = new Cma(config);

    await expect(cma.listEnvironmentProfiles()).resolves.toHaveLength(1);
    await expect(cma.listEnvironmentProfilesPage({ limit: 1 })).resolves.toMatchObject({
      items: [{ id: 'envp_1' }],
    });
    await expect(cma.readSkillCatalogue()).resolves.toMatchObject({ skills: expect.any(Array) });
  });
});
