// @vitest-environment node
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { createServer, type RequestListener } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  request,
  type APIRequestContext,
  type Browser,
  type Page,
  type Route,
} from '@playwright/test';
import type {
  FullConfig,
  FullResult,
  Suite,
  TestCase,
  TestResult,
} from '@playwright/test/reporter';
import {
  assertOwnerCase,
  assertOwnerSession,
  ownerCapabilities,
  ownerCase,
  ownerFixture,
  ownerScope,
  ownerSettings,
} from './owner-environment';
import { ownerRead, validateOwnerPrerequisites } from './owner-preflight';
import { OwnerHarness, ownerMutationAllowed } from './owner-harness';
import OwnerReporter from './owner-reporter';
import { prepareEvidenceDirectory } from './security';
import { Journal } from './journal';

function inputs() {
  const directory = mkdtempSync(join(tmpdir(), 'cma-owner-mechanics-'));
  const manifest = join(directory, 'fixture.json');
  const state = join(directory, 'state.json');
  const sentinels = join(directory, 'sentinels.json');
  writeFileSync(state, JSON.stringify({ cookies: [], origins: [] }), { mode: 0o600 });
  writeFileSync(sentinels, JSON.stringify(['redacted-owner-probe']), { mode: 0o600 });
  const document = {
    origin: 'https://owner.example.test',
    subject: 'owner-fixture',
    storageState: state,
    forbiddenValuesFile: sentinels,
    profiles: {
      sandbox: { id: 'sbp_owner', revision: 1, digest: 'sandbox-reviewed' },
      workspace: { id: 'wkp_owner', revision: 2, digest: 'workspace-reviewed' },
      agent: { id: 'agp_owner', revision: 3, digest: 'agent-reviewed' },
    },
    provenance: {
      commit: 'a'.repeat(40),
      nyxidCommit: 'b'.repeat(40),
      apiImage: `registry.example.test/api@sha256:${'c'.repeat(64)}`,
      webImage: `registry.example.test/web@sha256:${'d'.repeat(64)}`,
      runtimeImage: `registry.example.test/runtime@sha256:${'e'.repeat(64)}`,
    },
  };
  const save = () => writeFileSync(manifest, JSON.stringify(document), { mode: 0o600 });
  save();
  const env = {
    CMA_OWNER_E2E_TARGET: document.origin,
    CMA_OWNER_E2E_MUTATION_TARGET: document.origin,
    CMA_OWNER_E2E_SUBJECT: document.subject,
    CMA_OWNER_E2E_FIXTURE_FILE: manifest,
    CMA_OWNER_E2E_OUTPUT_DIR: join(directory, 'evidence'),
  };
  const config = ownerSettings(env);
  return { directory, manifest, state, sentinels, document, env, config, save };
}

function publicReads(fixture: ReturnType<typeof ownerFixture>) {
  const rows = new Map<string, unknown>();
  rows.set('/auth/session', {
    authenticated: true,
    visitor: false,
    subject: fixture.subject,
    capabilities: [...ownerCapabilities],
  });
  for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
    const pin = fixture.profiles[kind];
    const path = `/api/v1/${kind}-profiles/${pin.id}`;
    rows.set(path, { id: pin.id, owner: fixture.subject });
    rows.set(`${path}/revisions/${pin.revision}`, {
      profile_id: pin.id,
      revision: pin.revision,
      digest: pin.digest,
      published_at: '2026-09-07T00:00:00Z',
      content:
        kind === 'sandbox'
          ? {
              workspace_profiles: [],
              agent_provider: 'codex',
              model_access: 'chronoai_managed',
              max_workspaces: 1,
              max_agents_per_workspace: 1,
            }
          : kind === 'workspace'
            ? { agent_profiles: [] }
            : {},
    });
  }
  const get = vi.fn(async (url: string) => ({
    ok: () => rows.has(new URL(url).pathname),
    text: async () => JSON.stringify(rows.get(new URL(url).pathname)),
  }));
  return { rows, get, api: { get } as unknown as APIRequestContext };
}

function ownerBrowser(session: Record<string, unknown>) {
  const fixture = inputs();
  for (const [name, value] of Object.entries(fixture.env)) vi.stubEnv(name, value);
  const reads = publicReads(ownerFixture(fixture.config));
  reads.rows.set('/auth/session', { ...(reads.rows.get('/auth/session') as object), ...session });
  const route = vi
    .fn<(pattern: string, handler: (route: Route) => Promise<void>) => Promise<void>>()
    .mockResolvedValue(undefined);
  const context = {
    request: reads.api,
    route,
    on: vi.fn(),
    newPage: vi.fn().mockResolvedValue({}),
  };
  const browser = { newContext: vi.fn().mockResolvedValue(context) } as unknown as Browser;
  return { owner: new OwnerHarness(browser), context, reads };
}

function browserRoute(url: string, method = 'GET', resourceType = 'image') {
  const abort = vi.fn().mockResolvedValue(undefined);
  const proceed = vi.fn().mockResolvedValue(undefined);
  return {
    abort,
    proceed,
    route: {
      request: () => ({ url: () => url, method: () => method, resourceType: () => resourceType }),
      abort,
      continue: proceed,
    } as unknown as Route,
  };
}

async function loopback(handler: RequestListener) {
  const server = createServer(handler);
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('Missing loopback port');
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      }),
  };
}

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe('owner-only fail-closed inputs', () => {
  it('requires explicit exact-target mutation opt-in without weakening the full campaign', () => {
    const fixture = inputs();
    expect(ownerFixture(fixture.config).subject).toBe(fixture.document.subject);
    expect(() =>
      ownerSettings({ ...fixture.env, CMA_OWNER_E2E_MUTATION_TARGET: undefined }),
    ).toThrow('exact HTTPS origin');
    expect(() =>
      ownerSettings({
        ...fixture.env,
        CMA_OWNER_E2E_MUTATION_TARGET: 'https://other.example.test',
      }),
    ).toThrow('exact HTTPS origin');
    expect(() =>
      ownerSettings({ ...fixture.env, CMA_OWNER_E2E_TARGET: 'http://owner.example.test' }),
    ).toThrow('HTTPS');
    fixture.document.origin = 'https://other.example.test';
    fixture.save();
    expect(() => ownerFixture(fixture.config)).toThrow('origin');
  });
  it('rejects the wrong configured account', () => {
    const fixture = inputs();
    fixture.document.subject = 'other-owner';
    fixture.save();
    expect(() => ownerFixture(fixture.config)).toThrow('account');
  });
  it.each(['manifest', 'state', 'sentinels'] as const)(
    'rejects insecure %s input permissions',
    (field) => {
      const fixture = inputs();
      chmodSync(fixture[field], 0o640);
      expect(() => ownerFixture(ownerSettings(fixture.env))).toThrow('0600');
    },
  );
  it('rejects symlinks, repository inputs and existing evidence leaves', () => {
    const fixture = inputs();
    const link = join(fixture.directory, 'linked-state');
    symlinkSync(fixture.state, link);
    fixture.document.storageState = link;
    fixture.save();
    expect(() => ownerFixture(fixture.config)).toThrow('regular 0600');
    expect(() =>
      ownerSettings({ ...fixture.env, CMA_OWNER_E2E_FIXTURE_FILE: process.cwd() }),
    ).toThrow('outside the repository');
    prepareEvidenceDirectory(fixture.config.output, 'owner-mechanics-run');
    expect(() => prepareEvidenceDirectory(fixture.config.output, 'different-owner-run')).toThrow(
      'fresh private',
    );
  });
  it('rejects unreviewed revisions and mutable provenance', () => {
    const fixture = inputs();
    fixture.document.profiles.agent.revision = 0;
    fixture.save();
    expect(() => ownerFixture(fixture.config)).toThrow('revision');
    fixture.document.profiles.agent.revision = 3;
    fixture.document.provenance.runtimeImage = 'registry.example.test/runtime:latest';
    fixture.save();
    expect(() => ownerFixture(fixture.config)).toThrow('immutable pins');
  });
});

describe('owner-only read preflight', () => {
  it('checks one real subject and exact owner-published revisions using GET with no redirects', async () => {
    const fixture = ownerFixture(inputs().config);
    const mock = publicReads(fixture);
    await validateOwnerPrerequisites(mock.api, fixture, []);
    expect(mock.get).toHaveBeenCalledTimes(7);
    for (const [url, options] of mock.get.mock.calls as unknown as [string, unknown][])
      expect({ url, options }).toEqual({
        url: expect.stringMatching(/^https:\/\/owner\.example\.test\//),
        options: { maxRedirects: 0, timeout: 30_000 },
      });
  });
  it.each([{ authenticated: false }, { subject: 'other-owner' }, { visitor: true }])(
    'refuses identity mismatch before reading profile inputs %j',
    async (override) => {
      const fixture = ownerFixture(inputs().config);
      const mock = publicReads(fixture);
      mock.rows.set('/auth/session', {
        ...(mock.rows.get('/auth/session') as object),
        ...override,
      });
      await expect(validateOwnerPrerequisites(mock.api, fixture, [])).rejects.toThrow(
        'existing owner account',
      );
      expect(mock.get).toHaveBeenCalledOnce();
    },
  );
  it.each(ownerCapabilities)(
    'refuses missing %s capability before later network reads',
    async (missing) => {
      const fixture = ownerFixture(inputs().config);
      const mock = publicReads(fixture);
      mock.rows.set('/auth/session', {
        authenticated: true,
        visitor: false,
        subject: fixture.subject,
        capabilities: ownerCapabilities.filter((name) => name !== missing),
      });
      await expect(validateOwnerPrerequisites(mock.api, fixture, [])).rejects.toThrow(
        'capabilities',
      );
      expect(mock.get).toHaveBeenCalledOnce();
    },
  );
  it.each(['owner', 'digest', 'children', 'provider-login'] as const)(
    'refuses wrong profile %s prerequisites',
    async (fault) => {
      const fixture = ownerFixture(inputs().config);
      const mock = publicReads(fixture);
      const path = `/api/v1/sandbox-profiles/${fixture.profiles.sandbox.id}`;
      if (fault === 'owner')
        mock.rows.set(path, { id: fixture.profiles.sandbox.id, owner: 'other-owner' });
      else {
        const revision = mock.rows.get(`${path}/revisions/1`) as {
          digest: string;
          content: Record<string, unknown>;
        };
        if (fault === 'digest') revision.digest = 'different-digest';
        if (fault === 'children')
          revision.content.workspace_profiles = [{ id: 'automatic', revision: 1 }];
        if (fault === 'provider-login') revision.content.model_access = 'own_credentials';
      }
      await expect(validateOwnerPrerequisites(mock.api, fixture, [])).rejects.toThrow('E2E owner');
    },
  );
  it('rejects failed reads and configured credential sentinels without returning their content', async () => {
    const fixture = ownerFixture(inputs().config);
    const mock = publicReads(fixture);
    await expect(ownerRead(mock.api, fixture.origin, '/api/v1/absent', [])).rejects.toThrow(
      'public read unavailable',
    );
    await expect(ownerRead(mock.api, fixture.origin, '/auth/login', [])).rejects.toThrow(
      'read path refused',
    );
    mock.rows.set('/auth/session', { value: 'redacted-owner-probe' });
    await expect(
      ownerRead(mock.api, fixture.origin, '/auth/session', ['redacted-owner-probe']),
    ).rejects.toThrow('Credential material');
    expect(() => assertOwnerSession({}, fixture.subject)).toThrow('owner account');
  });
});

describe('owner-only cosmetic avatar blocking', () => {
  const avatar = 'https://images.example.test/owner-avatar.png?size=96';

  it('blocks the exact verified session avatar GET image without a fatal violation', async () => {
    const { owner, context } = ownerBrowser({ avatar_url: avatar });
    await owner.open();
    const request = browserRoute(avatar);
    await context.route.mock.calls[0]![1](request.route);
    expect(request.abort).toHaveBeenCalledExactlyOnceWith('blockedbyclient');
    expect(request.proceed).not.toHaveBeenCalled();
    expect(owner.violations).toEqual([]);
  });

  it.each([
    [avatar.replace('owner-avatar', 'other-avatar'), 'GET', 'image'],
    [avatar.replace('size=96', 'size=97'), 'GET', 'image'],
    [`${avatar}&extra=1`, 'GET', 'image'],
    [avatar.replace('images.example.test', 'other.example.test'), 'GET', 'image'],
    [avatar, 'HEAD', 'image'],
    [avatar, 'POST', 'image'],
    [avatar, 'GET', 'document'],
    [avatar, 'GET', 'script'],
    [avatar, 'GET', 'fetch'],
    ['https://owner.example.test/admin', 'GET', 'document'],
    ['https://owner.example.test/auth/login', 'GET', 'document'],
  ])('keeps %s %s %s fatal and blocked', async (url, method, resourceType) => {
    const { owner, context } = ownerBrowser({ avatar_url: avatar });
    await owner.open();
    const request = browserRoute(url, method, resourceType);
    await context.route.mock.calls[0]![1](request.route);
    expect(request.abort).toHaveBeenCalledExactlyOnceWith('blockedbyclient');
    expect(request.proceed).not.toHaveBeenCalled();
    expect(owner.violations).toEqual(['out_of_scope_browser_request']);
  });

  it.each([
    undefined,
    null,
    42,
    'not-a-url',
    'http://images.example.test/avatar',
    'https://user:password@images.example.test/avatar',
  ])('does not trust invalid avatar metadata %s', async (avatar_url) => {
    const { owner, context } = ownerBrowser({ avatar_url });
    await owner.open();
    const request = browserRoute(
      typeof avatar_url === 'string' && avatar_url.includes('://') ? avatar_url : avatar,
    );
    await context.route.mock.calls[0]![1](request.route);
    expect(request.abort).toHaveBeenCalledExactlyOnceWith('blockedbyclient');
    expect(request.proceed).not.toHaveBeenCalled();
    expect(owner.violations).toEqual(['out_of_scope_browser_request']);
  });

  it('binds each cosmetic exception to its context live session', async () => {
    const { owner, context, reads } = ownerBrowser({ avatar_url: avatar });
    await owner.open();
    const replacement = avatar.replace('owner-avatar', 'replacement-avatar');
    reads.rows.set('/auth/session', {
      ...(reads.rows.get('/auth/session') as object),
      avatar_url: replacement,
    });
    await owner.open();
    for (const [index, url] of [
      [0, avatar],
      [1, replacement],
    ] as const) {
      const request = browserRoute(url);
      await context.route.mock.calls[index]![1](request.route);
      expect(request.abort).toHaveBeenCalled();
      expect(request.proceed).not.toHaveBeenCalled();
    }
    expect(owner.violations).toEqual([]);
    await context.route.mock.calls[1]![1](browserRoute(avatar).route);
    expect(owner.violations).toEqual(['out_of_scope_browser_request']);
  });

  it('does not install any exception before live identity verification', async () => {
    const { owner, context } = ownerBrowser({ avatar_url: avatar, subject: 'other-owner' });
    await expect(owner.open()).rejects.toThrow('existing owner account');
    expect(context.route).not.toHaveBeenCalled();
    expect(context.newPage).not.toHaveBeenCalled();
  });
});

describe('owner-only mutation and cleanup mechanics', () => {
  it('allows only normal lifecycle POSTs for the exact run-owned Sandbox', () => {
    const owned = [{ kind: 'sandboxes' as const, id: 'sbx_run', subject: 'owner-fixture' }];
    expect(ownerMutationAllowed('/api/v1/sandboxes/sbx_run/pause', 'POST', owned)).toBe(true);
    expect(ownerMutationAllowed('/api/v1/sandboxes/sbx_run/revive', 'POST', owned)).toBe(true);
    for (const path of [
      '/api/v1/sandboxes/sbx_other/pause',
      '/api/v1/admin/settings',
      '/api/v1/agents/agt_run/input',
      '/api/v1/sandboxes',
    ])
      expect(ownerMutationAllowed(path, 'POST', owned)).toBe(false);
    expect(ownerMutationAllowed('/api/v1/sandboxes/sbx_run/pause', 'DELETE', owned)).toBe(false);
  });
  it('refuses an unowned parent before any creation request', async () => {
    const fixture = inputs();
    for (const [name, value] of Object.entries(fixture.env)) vi.stubEnv(name, value);
    const owner = new OwnerHarness({} as Browser);
    await expect(owner.create({} as Page, 'agent', 'wks_other')).rejects.toThrow('not owned');
    expect(owner.commands).toEqual([]);
  });
  it('retains cleanup ownership after an ambiguous creation instead of deleting a possible parent', async () => {
    const fixture = inputs();
    for (const [name, value] of Object.entries(fixture.env)) vi.stubEnv(name, value);
    prepareEvidenceDirectory(fixture.config.output, 'owner-mechanics-run')();
    const owner = new OwnerHarness({} as Browser);
    owner.journal.own('sandboxes', 'sbx_run', fixture.document.subject);
    const mock = publicReads(owner.fixture);
    const post = vi.fn().mockRejectedValue(new TypeError('response lost'));
    const page = { request: { post }, context: () => ({ request: mock.api }) } as unknown as Page;
    await expect(owner.create(page, 'workspace', 'sbx_run')).rejects.toThrow('response lost');
    const cleanup = vi.spyOn(owner.journal, 'cleanup');
    await expect(owner.dispose()).rejects.toThrow('admission is unresolved');
    expect(cleanup).not.toHaveBeenCalled();
    const log = readFileSync(join(fixture.config.output, 'cleanup.ndjson'), 'utf8');
    expect(log).toContain('creation_requested');
    expect(log).toContain('unresolved_owner_creation');
    expect(owner.journal.resources).toHaveLength(1);
  });
  it.each([408, 503])(
    'retains cleanup ownership after HTTP %s creation uncertainty',
    async (status) => {
      const fixture = inputs();
      for (const [name, value] of Object.entries(fixture.env)) vi.stubEnv(name, value);
      prepareEvidenceDirectory(fixture.config.output, 'owner-mechanics-run')();
      const owner = new OwnerHarness({} as Browser);
      owner.journal.own('sandboxes', 'sbx_run', fixture.document.subject);
      const mock = publicReads(owner.fixture);
      const page = {
        request: {
          post: vi.fn().mockResolvedValue({
            status: () => status,
            text: async () => JSON.stringify({ code: 'unavailable' }),
          }),
        },
        context: () => ({ request: mock.api }),
      } as unknown as Page;
      await expect(owner.create(page, 'workspace', 'sbx_run')).rejects.toThrow('not confirmed');
      const cleanup = vi.spyOn(owner.journal, 'cleanup');
      await expect(owner.dispose()).rejects.toThrow('admission is unresolved');
      expect(cleanup).not.toHaveBeenCalled();
    },
  );
});

describe('shared cleanup transport bounds', () => {
  it('bounds every identity, quiescence, stop, delete, retire and completion request', async () => {
    const fixture = inputs();
    const subject = fixture.document.subject;
    const journal = new Journal(fixture.directory, 'cleanup-bounds');
    journal.own('launches', 'lch_run', subject);
    journal.own('agents', 'agt_run', subject);
    journal.own('agent-profiles', 'agp_run', subject);
    journal.own('sandboxes', 'sbx_other', 'other-owner');
    const response = (body: unknown, status = 200) => ({
      ok: () => status >= 200 && status < 300,
      status: () => status,
      json: async () => body,
    });
    const get = vi
      .fn()
      .mockResolvedValueOnce(response({ subject }))
      .mockResolvedValueOnce(response({ id: 'lch_run', state: 'completed' }))
      .mockResolvedValueOnce(response({ owner: subject, status: 'ready' }))
      .mockResolvedValueOnce(response({ owner: subject, status: 'stopped' }))
      .mockResolvedValueOnce(response({}, 404))
      .mockResolvedValueOnce(response({ owner: subject, availability: 'published' }))
      .mockResolvedValueOnce(response({ owner: subject, availability: 'retired' }));
    const post = vi.fn().mockResolvedValue(response({}));
    const fetch = vi.fn().mockResolvedValue(response({}));
    await journal.cleanup(
      { get, post, fetch } as unknown as APIRequestContext,
      fixture.document.origin,
      subject,
    );
    expect(get).toHaveBeenCalledTimes(7);
    expect(post.mock.calls.map(([url]) => new URL(url).pathname)).toEqual([
      '/api/v1/launches/lch_run/cancel',
      '/api/v1/agents/agt_run/stop',
    ]);
    expect(
      fetch.mock.calls.map(([url, options]) => [new URL(url).pathname, options.method]),
    ).toEqual([
      ['/api/v1/agents/agt_run', 'DELETE'],
      ['/api/v1/agent-profiles/agp_run/retire', 'POST'],
    ]);
    for (const [url, options] of [...get.mock.calls, ...post.mock.calls, ...fetch.mock.calls]) {
      expect(url).not.toContain('sbx_other');
      expect(options).toMatchObject({ maxRedirects: 0, timeout: 30_000 });
    }
  });

  describe.each([307, 308])('HTTP %s redirects', (status) => {
    it.each(['identity', 'read', 'stop', 'delete', 'retire'] as const)(
      'never follows a cleanup %s redirect to another origin',
      async (stage) => {
        const fixture = inputs();
        const subject = fixture.document.subject;
        const journal = new Journal(fixture.directory, 'cleanup-redirect');
        const kind =
          stage === 'retire' ? 'agent-profiles' : stage === 'stop' ? 'agents' : 'sandboxes';
        journal.own(kind, 'resource_run', subject);
        const resourcePath = `/api/v1/${kind}/resource_run`;
        const redirectPath =
          stage === 'identity'
            ? '/auth/session'
            : `${resourcePath}${stage === 'stop' ? '/stop' : stage === 'retire' ? '/retire' : ''}`;
        const redirectMethod =
          stage === 'delete' ? 'DELETE' : ['stop', 'retire'].includes(stage) ? 'POST' : 'GET';
        const escaped: string[] = [];
        const destination = await loopback((incoming, response) => {
          escaped.push(`${incoming.method} ${incoming.url}`);
          response.writeHead(403, { 'Content-Type': 'application/json' });
          response.end(JSON.stringify({ code: 'redirect_destination_reached' }));
        });
        let source: Awaited<ReturnType<typeof loopback>> | undefined;
        let api: APIRequestContext | undefined;
        const observed: string[] = [];
        try {
          source = await loopback((incoming, response) => {
            observed.push(`${incoming.method} ${incoming.url}`);
            if (incoming.url === redirectPath && incoming.method === redirectMethod) {
              response.writeHead(status, { Location: `${destination.origin}/escaped` });
              response.end();
            } else {
              response.writeHead(200, { 'Content-Type': 'application/json' });
              response.end(
                JSON.stringify(
                  incoming.url === '/auth/session'
                    ? { subject }
                    : { owner: subject, status: 'ready', availability: 'published' },
                ),
              );
            }
          });
          api = await request.newContext();
          await expect(journal.cleanup(api, source.origin, subject)).rejects.toThrow(
            stage === 'identity' ? 'Cleanup refused' : 'Scoped cleanup is incomplete',
          );
          expect(observed.at(-1)).toBe(`${redirectMethod} ${redirectPath}`);
          expect(escaped).toEqual([]);
          expect(journal.resources).toEqual([{ kind, id: 'resource_run', subject }]);
          expect(readFileSync(join(fixture.directory, 'cleanup.ndjson'), 'utf8')).toContain(
            stage === 'identity'
              ? 'cleanup_refused'
              : stage === 'read'
                ? 'cleanup_read_failed'
                : stage === 'stop'
                  ? 'stop_requested'
                  : 'cleanup_failed',
          );
        } finally {
          await api?.dispose();
          await source?.close();
          await destination.close();
        }
      },
    );
  });
});

describe('owner-only evidence cannot become a full-campaign pass', () => {
  it('labels discovery as no remote execution and creates no results', () => {
    const output = vi.spyOn(process.stdout, 'write').mockReturnValue(true);
    new OwnerReporter().onEnd({ status: 'passed' } as FullResult);
    expect(output).toHaveBeenCalledWith(expect.stringContaining('no remote scenarios executed'));
  });
  it.each(['J0 signup', 'J6 lifecycle', 'G-S2 visitor', 'OWNER-unreviewed extra case'])(
    'rejects out-of-scope title %s',
    (title) => {
      expect(() => assertOwnerCase(title)).toThrow('case labels');
    },
  );
  it('reports only validated current-run owner evidence with every full gate deferred', () => {
    const fixture = inputs();
    for (const [name, value] of Object.entries(fixture.env)) vi.stubEnv(name, value);
    vi.stubEnv('CMA_OWNER_E2E_RUN', 'owner-evidence-current-run');
    vi.spyOn(process.stdout, 'write').mockReturnValue(true);
    const reporter = new OwnerReporter();
    reporter.onBegin(
      {} as FullConfig,
      { allTests: () => [{ title: ownerCase }] } as unknown as Suite,
    );
    reporter.onTestEnd(
      { title: ownerCase } as TestCase,
      { status: 'passed', duration: 1 } as TestResult,
    );
    reporter.onEnd({ status: 'passed' } as FullResult);
    const result = join(fixture.config.output, 'owner-results.json');
    expect(existsSync(result)).toBe(false);
    prepareEvidenceDirectory(fixture.config.output, 'owner-evidence-current-run')();
    reporter.onEnd({ status: 'passed' } as FullResult);
    const evidence = JSON.parse(readFileSync(result, 'utf8'));
    expect(evidence).toEqual({
      ...ownerScope,
      status: 'passed',
      cases: [{ case: ownerCase, status: 'passed', durationMs: 1 }],
    });
    expect(evidence).toMatchObject({
      identityClass: 'existing_owner',
      accountCount: 1,
      scope: 'owner_only_partial',
      fullCampaignGatesAdvanced: false,
    });
    expect(evidence.deferredGates).toEqual(
      expect.arrayContaining(['J0', 'J5', 'J6', 'G-S2', 'G-T3', 'cross_owner_isolation']),
    );
  });
});
