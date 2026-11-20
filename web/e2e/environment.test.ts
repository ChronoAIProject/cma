// @vitest-environment node
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it, vi } from 'vitest';
import {
  faultNames,
  fixtures,
  identifier,
  object,
  origin,
  outsideRepository,
  settings,
  visitorCases,
} from './environment';
import {
  assertCredentialFree,
  credentialValues,
  evidenceDirectoryIsReady,
  isJsonMediaType,
  prepareEvidenceDirectory,
  readOnlyGuard,
} from './security';
import { Journal, launchIsQuiescent } from './journal';
import RedactedReporter from './reporter';
import { tc117Fixture, instanceOrigins } from './visitor-abuse-environment';
import type { APIRequestContext, BrowserContext, Route } from '@playwright/test';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';

function fixtureDocument() {
  const directory = mkdtempSync(join(tmpdir(), 'cma-e2e-fixture-'));
  const state = join(directory, 'state.json');
  const manifest = join(directory, 'manifest.json');
  const forbidden = join(directory, 'forbidden.json');
  const hooksFile = join(directory, 'hooks.mjs');
  writeFileSync(state, JSON.stringify({ cookies: [], origins: [] }), { mode: 0o600 });
  writeFileSync(forbidden, JSON.stringify(['redacted-probe']), { mode: 0o600 });
  writeFileSync(hooksFile, 'export {};', { mode: 0o600 });
  const trigger = { publicId: 'public-fixture', placementId: 'placement-fixture' };
  const document = {
    origin: 'https://staging.example.test',
    fixtureOwner: 'fixture-owner',
    baselineServices: ['model'],
    accounts: {
      owner: { subject: 'fixture-owner', storageState: state },
      secondOwner: { subject: 'second-owner', storageState: state },
    },
    visitors: Object.fromEntries(
      visitorCases.map((name, index) => [
        name,
        { subject: `visitor-${index}`, storageState: state },
      ]),
    ),
    triggers: Object.fromEntries(
      [
        'fresh',
        'newWorkspace',
        'ask',
        'resume',
        'missingService',
        'retired',
        'paused',
        'quota',
      ].map((name) => [name, trigger]),
    ),
    connections: Object.fromEntries(
      ['oauth', 'api_key', 'device_code'].map((name) => [name, trigger]),
    ),
    faults: Object.fromEntries(
      faultNames.map((name) => [name, { ...trigger, expectedCode: 'expected_failure' }]),
    ),
    profiles: Object.fromEntries(
      ['agent', 'workspace', 'sandbox'].map((kind) => [
        kind,
        { id: `${kind}-profile`, revision: 1, name: 'Public fixture' },
      ]),
    ),
    embeds: Object.fromEntries(
      ['markdown', 'html', 'javascript', 'react'].map((kind) => [
        kind,
        { url: `https://embed.example.test/${kind}`, placementId: 'placement-fixture' },
      ]),
    ),
    provenance: {
      commit: 'a'.repeat(40),
      nyxidCommit: 'b'.repeat(40),
      apiImage: `registry.example.test/api@sha256:${'c'.repeat(64)}`,
      webImage: `registry.example.test/web@sha256:${'d'.repeat(64)}`,
      runtimeImage: `registry.example.test/runtime@sha256:${'e'.repeat(64)}`,
    },
    hooksFile,
    forbiddenValuesFile: forbidden,
  };
  writeFileSync(manifest, JSON.stringify(document), { mode: 0o600 });
  const env = {
    CMA_E2E_BASE_URL: document.origin,
    CMA_E2E_PUBLIC_ID: trigger.publicId,
    CMA_E2E_OUTPUT_DIR: join(directory, 'output'),
    CMA_E2E_MODE: 'mutation',
    CMA_E2E_MUTATION_TARGET: document.origin,
    CMA_E2E_FIXTURE_OWNER: document.fixtureOwner,
    CMA_E2E_FIXTURE_FILE: manifest,
  };
  return { document, env, manifest, state, directory };
}

describe('opt-in TC-117 prerequisites', () => {
  it('leaves every existing fixture scenario unchanged without TC-117 accounts', () => {
    const fixture = fixtureDocument();
    expect(Object.keys(fixtures(fixture.env).visitors)).toEqual(visitorCases);
    writeFileSync(fixture.manifest, JSON.stringify({ ...fixture.document, tc117: 'invalid' }));
    expect(Object.keys(fixtures(fixture.env).visitors)).toEqual(visitorCases);
    expect(() => tc117Fixture(fixture.env)).toThrow('tc117');
  });
  it('fails explicitly when the selected campaign has no TC-117 section', () => {
    expect(() => tc117Fixture(fixtureDocument().env)).toThrow('tc117');
  });
  it('defers absent instance origins but refuses malformed or duplicate pairs', () => {
    expect(instanceOrigins(undefined)).toEqual([]);
    expect(instanceOrigins('https://one.example.test,https://two.example.test')).toEqual([
      'https://one.example.test',
      'https://two.example.test',
    ]);
    for (const value of [
      '',
      'https://one.example.test',
      'https://one.example.test,https://one.example.test/',
      'https://one.example.test,http://two.example.test',
      'https://one.example.test,https://two.example.test/path',
      'https://one.example.test,https://two.example.test,https://three.example.test',
    ])
      expect(() => instanceOrigins(value)).toThrow();
  });
});

describe('staging harness fail-closed prerequisites', () => {
  const valid = {
    CMA_E2E_BASE_URL: 'https://staging.example.test',
    CMA_E2E_PUBLIC_ID: 'public-fixture',
    CMA_E2E_OUTPUT_DIR: '/tmp/cma-e2e-guard-test',
  };
  it('defaults to read-only smoke and never silently selects mutation', () => {
    expect(settings(valid).mode).toBe('smoke');
    expect(() => settings({ ...valid, CMA_E2E_MODE: 'other' })).toThrow('Invalid');
    expect(() => settings({ ...valid, CMA_E2E_MODE: 'mutation' })).toThrow('exact base origin');
    expect(() =>
      settings({
        ...valid,
        CMA_E2E_MODE: 'mutation',
        CMA_E2E_MUTATION_TARGET: 'https://other.example.test',
      }),
    ).toThrow('exact base origin');
    expect(
      settings({
        ...valid,
        CMA_E2E_MODE: 'mutation',
        CMA_E2E_MUTATION_TARGET: valid.CMA_E2E_BASE_URL,
      }).mode,
    ).toBe('mutation');
  });
  it.each([
    'http://staging.example.test',
    'https://user@example.test',
    'https://example.test/api',
    'https://example.test?token=example',
    'https://example.test#fragment',
  ])('refuses unsafe origin %s', (value) => {
    expect(() => origin(value, 'target')).toThrow();
  });
  it('requires a named origin, public ID and output location before networking', () => {
    expect(() => settings({})).toThrow('CMA_E2E_BASE_URL');
    expect(() => settings({ CMA_E2E_BASE_URL: valid.CMA_E2E_BASE_URL })).toThrow(
      'CMA_E2E_PUBLIC_ID',
    );
    expect(() => identifier('../arbitrary', 'ID')).toThrow('identifier');
    expect(() => identifier('', 'ID')).toThrow('prerequisite');
    expect(() => object([])).toThrow('Invalid');
    expect(() => object(null)).toThrow('Invalid');
  });
  it('keeps credentials and evidence outside the repository', () => {
    expect(() => outsideRepository('relative.json', 'auth state')).toThrow('absolute path');
    expect(() => outsideRepository(process.cwd(), 'auth state')).toThrow('outside the repository');
    expect(outsideRepository('/tmp/cma-e2e-test', 'output')).toBe('/tmp/cma-e2e-test');
  });
  it('allows a sibling of the checkout while rejecting the checkout and nested paths', () => {
    const root = fileURLToPath(new URL('../..', import.meta.url));
    const sibling = fileURLToPath(new URL('../../../cma-e2e-sibling-output', import.meta.url));
    expect(outsideRepository(sibling, 'output')).toBe(sibling);
    expect(() => outsideRepository(root, 'output')).toThrow('outside the repository');
    expect(() => outsideRepository(join(root, 'web', 'nested-evidence'), 'output')).toThrow(
      'outside the repository',
    );
  });
});

describe('complete mutation fixtures', () => {
  it('requires every scenario and pinned provenance before a run can begin', () => {
    const fixture = fixtureDocument();
    expect(Object.keys(fixtures(fixture.env).visitors)).toEqual(visitorCases);
    for (const key of [
      'accounts',
      'visitors',
      'triggers',
      'connections',
      'faults',
      'profiles',
      'embeds',
      'provenance',
      'baselineServices',
    ]) {
      const body: Record<string, unknown> = { ...fixture.document };
      delete body[key];
      writeFileSync(fixture.manifest, JSON.stringify(body));
      expect(() => fixtures(fixture.env)).toThrow();
    }
  });
  it('refuses a different owner, shared visitor identities and unpinned images', () => {
    const fixture = fixtureDocument();
    expect(() => fixtures({ ...fixture.env, CMA_E2E_FIXTURE_OWNER: 'wrong-owner' })).toThrow(
      'fixture owner',
    );
    fixture.document.visitors.J2!.subject = fixture.document.visitors.J1!.subject;
    writeFileSync(fixture.manifest, JSON.stringify(fixture.document));
    expect(() => fixtures(fixture.env)).toThrow('distinct identity');
    fixture.document.visitors.J2!.subject = 'distinct-visitor';
    fixture.document.provenance.runtimeImage = 'registry.example.test/runtime:latest';
    writeFileSync(fixture.manifest, JSON.stringify(fixture.document));
    expect(() => fixtures(fixture.env)).toThrow('pinned provenance');
  });
  it('refuses group-readable auth state and symlinks into the checkout', () => {
    const fixture = fixtureDocument();
    chmodSync(fixture.state, 0o640);
    expect(() => fixtures(fixture.env)).toThrow('0600');
    const alias = join(fixture.directory, 'checkout');
    symlinkSync(process.cwd(), alias);
    expect(() => outsideRepository(join(alias, 'e2e'), 'output')).toThrow('outside the repository');
  });
});

describe('credential-safe observations', () => {
  it('requires an exclusive output leaf and a completed setup marker for the current run', () => {
    const parent = mkdtempSync(join(tmpdir(), 'cma-e2e-output-'));
    const output = join(parent, 'new-evidence');
    const run = 'current-evidence-run';
    const complete = prepareEvidenceDirectory(output, run);
    expect(evidenceDirectoryIsReady(output, run)).toBe(false);
    complete();
    expect(evidenceDirectoryIsReady(output, run)).toBe(true);
    expect(evidenceDirectoryIsReady(output, 'different-run-identity')).toBe(false);
    expect(evidenceDirectoryIsReady(output, undefined)).toBe(false);
    writeFileSync(join(output, 'unrelated.txt'), 'preserved');
    expect(() => prepareEvidenceDirectory(output, run)).toThrow('fresh private directory');
    expect(readFileSync(join(output, 'unrelated.txt'), 'utf8')).toBe('preserved');
    const linked = join(parent, 'linked-evidence');
    symlinkSync(output, linked);
    expect(() => prepareEvidenceDirectory(linked, run)).toThrow('fresh private directory');
    expect(evidenceDirectoryIsReady(linked, run)).toBe(false);
    chmodSync(output, 0o750);
    expect(evidenceDirectoryIsReady(output, run)).toBe(false);
  });
  it('does not let a reporter write to incomplete setup or another run directory', () => {
    const parent = mkdtempSync(join(tmpdir(), 'cma-e2e-output-'));
    const output = join(parent, 'new-evidence');
    const run = 'current-evidence-run';
    const complete = prepareEvidenceDirectory(output, run);
    vi.stubEnv('CMA_E2E_OUTPUT_DIR', output);
    vi.stubEnv('CMA_E2E_EVIDENCE_RUN', run);
    const write = vi.spyOn(process.stdout, 'write').mockReturnValue(true);
    try {
      const reporter = new RedactedReporter();
      reporter.onTestEnd(
        { title: 'static case' } as TestCase,
        { status: 'passed', duration: 1 } as TestResult,
      );
      const result = { status: 'failed' } as FullResult;
      reporter.onEnd(result);
      expect(existsSync(join(output, 'results.json'))).toBe(false);
      complete();
      vi.stubEnv('CMA_E2E_EVIDENCE_RUN', 'different-run-identity');
      reporter.onEnd(result);
      expect(existsSync(join(output, 'results.json'))).toBe(false);
      vi.stubEnv('CMA_E2E_EVIDENCE_RUN', run);
      reporter.onEnd(result);
      expect(JSON.parse(readFileSync(join(output, 'results.json'), 'utf8'))).toEqual({
        status: 'failed',
        cases: [{ case: 'static case', status: 'passed', durationMs: 1 }],
      });
    } finally {
      write.mockRestore();
      vi.unstubAllEnvs();
    }
  });
  it('screens standard and structured JSON media types, including problem responses', () => {
    for (const mediaType of [
      'application/json',
      'application/problem+json',
      'application/problem+json; charset=utf-8',
      'Application/Vnd.Cma+Json',
    ]) {
      expect(isJsonMediaType(mediaType)).toBe(true);
      expect(() => {
        if (isJsonMediaType(mediaType))
          assertCredentialFree(JSON.stringify({ detail: 'credential-probe-value' }), [
            'credential-probe-value',
          ]);
      }).toThrow('Credential material');
    }
    for (const mediaType of ['', 'text/html', 'text/event-stream', 'application/jsonp'])
      expect(isJsonMediaType(mediaType)).toBe(false);
  });
  it('refuses known sensitive values without printing them', () => {
    const sentinel = ['private', 'probe', 'value'].join('-');
    expect(() => assertCredentialFree(`message ${sentinel}`, [sentinel])).toThrow(
      'Credential material',
    );
    expect(() => assertCredentialFree('plain resource IDs and status', [sentinel])).not.toThrow();
    expect(() => assertCredentialFree(JSON.stringify({ refresh_token: 'redacted' }), [])).toThrow(
      'Credential material',
    );
  });
  it('requires nonempty sufficiently long sentinel values in a structured private file', () => {
    const directory = mkdtempSync(join(tmpdir(), 'cma-e2e-values-'));
    const path = join(directory, 'values.json');
    writeFileSync(path, JSON.stringify(['redacted-probe']), { mode: 0o600 });
    expect(credentialValues(path)).toEqual(['redacted-probe']);
    writeFileSync(path, '[]');
    expect(() => credentialValues(path)).toThrow('sentinels');
  });
  it('aborts all non-read browser methods in smoke mode', async () => {
    let handler: ((route: Route) => Promise<void>) | undefined;
    const context = {
      route: vi.fn(async (_pattern, callback) => {
        handler = callback;
      }),
    };
    const blocked: string[] = [];
    await readOnlyGuard(context as unknown as BrowserContext, blocked);
    for (const method of ['GET', 'HEAD', 'OPTIONS', 'POST', 'PUT', 'DELETE', 'PATCH']) {
      const route = {
        request: () => ({ method: () => method }),
        abort: vi.fn(),
        fallback: vi.fn(),
      };
      await handler!(route as unknown as Route);
      expect(route.abort).toHaveBeenCalledTimes(
        ['GET', 'HEAD', 'OPTIONS'].includes(method) ? 0 : 1,
      );
    }
    expect(blocked).toEqual(['POST', 'PUT', 'DELETE', 'PATCH']);
  });
});

describe('scoped resource journal', () => {
  it.each(['agent', 'workspace', 'sandbox'])(
    'journals only the real %s profile fork route',
    (kind) => {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.observe(
        `/api/v1/marketplace/${kind}-profiles/public-profile/fork`,
        'POST',
        201,
        { id: 'created-fork' },
        'owner-test',
      );
      journal.observe(
        `/api/v1/marketplace/${kind}/public-profile/fork`,
        'POST',
        201,
        { id: 'not-a-real-route' },
        'owner-test',
      );
      expect(journal.resources).toEqual([
        { kind: `${kind}-profiles`, id: 'created-fork', subject: 'owner-test' },
      ]);
    },
  );
  it('treats running, retryable and ambiguous terminal Launches as unsafe for cleanup', () => {
    for (const state of [
      'draft',
      'resolving',
      'sandbox_pending',
      'workspace_pending',
      'agent_pending',
      'message_pending',
      'failed_retryable',
    ])
      expect(launchIsQuiescent({ state })).toBe(false);
    for (const state of ['ready', 'completed']) expect(launchIsQuiescent({ state })).toBe(true);
    for (const state of ['cancelled', 'failed_terminal']) {
      expect(launchIsQuiescent({ state })).toBe(true);
      expect(
        launchIsQuiescent({
          state,
          started_at: 'recorded',
          resolution: {
            sandbox: { state: 'created' },
            workspace: { state: 'pending' },
            agent: { state: 'pending' },
          },
        }),
      ).toBe(false);
      expect(
        launchIsQuiescent({
          state,
          started_at: 'recorded',
          resolution: {
            sandbox: { state: 'created' },
            workspace: { state: 'reused' },
            agent: { state: 'created' },
          },
        }),
      ).toBe(true);
    }
  });
  it('tracks only admitted Launch resolutions and never owns reused resources', () => {
    const directory = mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-'));
    const journal = new Journal(directory, 'test-run');
    const body = {
      id: 'launch-test',
      resolution: {
        sandbox: { state: 'reused', id: 'existing-sandbox' },
        workspace: { state: 'created', id: 'workspace-test' },
        agent: { state: 'pending' },
      },
    };
    journal.observe('/api/v1/launches/launch-test', 'GET', 200, body, 'visitor-test');
    expect(journal.resources).toEqual([]);
    journal.observe(
      '/api/v1/triggers/public/public-test/launches',
      'POST',
      202,
      body,
      'visitor-test',
    );
    expect(journal.resources.map((row) => row.kind)).toEqual(['launches', 'workspaces']);
    journal.observe('/api/v1/launches/launch-test', 'GET', 200, body, 'visitor-test');
    expect(journal.resources).toHaveLength(2);
    expect(readFileSync(join(directory, 'cleanup.ndjson'), 'utf8')).not.toContain(
      'existing-sandbox',
    );
  });
  it('does not journal refusals or an arbitrary existing resource read', () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.observe('/api/v1/sandboxes', 'POST', 403, { id: 'not-created' }, 'visitor-test');
    journal.observe('/api/v1/sandboxes/existing', 'GET', 200, { id: 'existing' }, 'visitor-test');
    expect(journal.resources).toEqual([]);
    expect(() => journal.own('sandboxes', '../unsafe', 'visitor-test')).toThrow('identifier');
  });
  it('refuses cleanup before mutation if the live account differs', async () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.own('sandboxes', 'created-sandbox', 'visitor-test');
    const api = {
      get: vi
        .fn()
        .mockResolvedValue({ ok: () => true, json: async () => ({ subject: 'another-owner' }) }),
      fetch: vi.fn(),
      post: vi.fn(),
    };
    await expect(
      journal.cleanup(api as unknown as APIRequestContext, 'https://example.test', 'visitor-test'),
    ).rejects.toThrow('live identity');
    expect(api.fetch).not.toHaveBeenCalled();
    expect(api.post).not.toHaveBeenCalled();
  });
  it('stops only a run-created Agent before deletion and verifies its removal', async () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.own('agents', 'created-agent', 'visitor-test');
    const reply = (body: unknown, status = 200) => ({
      ok: () => status < 400,
      status: () => status,
      json: async () => body,
    });
    const api = {
      get: vi
        .fn()
        .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
        .mockResolvedValueOnce(reply({ id: 'created-agent', status: 'ready' }))
        .mockResolvedValueOnce(reply({ id: 'created-agent', status: 'stopped' }))
        .mockResolvedValueOnce(reply({}, 404)),
      post: vi.fn().mockResolvedValue(reply({}, 202)),
      fetch: vi.fn().mockResolvedValue(reply({}, 202)),
    };
    await journal.cleanup(
      api as unknown as APIRequestContext,
      'https://example.test',
      'visitor-test',
    );
    expect(api.post).toHaveBeenCalledWith(
      'https://example.test/api/v1/agents/created-agent/stop',
      expect.objectContaining({ data: {} }),
    );
    expect(api.fetch).toHaveBeenCalledWith(
      'https://example.test/api/v1/agents/created-agent',
      expect.objectContaining({ method: 'DELETE' }),
    );
  });
  it('harvests late children after cancel409 and waits for a quiescent Launch before deleting', async () => {
    vi.useFakeTimers();
    try {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.own('launches', 'created-launch', 'visitor-test');
      const reply = (body: unknown, status = 200) => ({
        ok: () => status < 400,
        status: () => status,
        json: async () => body,
      });
      const resolution = {
        sandbox: { state: 'reused', id: 'existing-sandbox' },
        workspace: { state: 'pending' },
        agent: { state: 'pending' },
      };
      const api = {
        get: vi
          .fn()
          .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
          .mockResolvedValueOnce(
            reply({ id: 'created-launch', state: 'workspace_pending', resolution }),
          )
          .mockResolvedValueOnce(
            reply({
              id: 'created-launch',
              state: 'completed',
              resolution: {
                ...resolution,
                workspace: { state: 'created', id: 'late-workspace' },
                agent: { state: 'reused', id: 'existing-agent' },
              },
            }),
          )
          .mockResolvedValueOnce(reply({ id: 'late-workspace', status: 'ready' }))
          .mockResolvedValueOnce(reply({}, 404)),
        post: vi.fn().mockResolvedValue(reply({}, 409)),
        fetch: vi.fn().mockResolvedValue(reply({}, 202)),
      };
      const cleanup = journal.cleanup(
        api as unknown as APIRequestContext,
        'https://example.test',
        'visitor-test',
      );
      await vi.advanceTimersByTimeAsync(0);
      expect(api.fetch).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1000);
      await cleanup;
      expect(api.fetch).toHaveBeenCalledTimes(1);
      expect(api.fetch).toHaveBeenCalledWith(
        'https://example.test/api/v1/workspaces/late-workspace',
        expect.objectContaining({ method: 'DELETE' }),
      );
    } finally {
      vi.useRealTimers();
    }
  });
  it.each(['cancelled', 'failed_terminal'])(
    'refuses child cleanup for a started %s Launch whose side effects remain ambiguous',
    async (state) => {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.own('launches', 'created-launch', 'visitor-test');
      const reply = (body: unknown) => ({
        ok: () => true,
        status: () => 200,
        json: async () => body,
      });
      const api = {
        get: vi
          .fn()
          .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
          .mockResolvedValueOnce(
            reply({
              id: 'created-launch',
              state,
              started_at: 'recorded',
              resolution: {
                sandbox: { state: 'created', id: 'created-sandbox' },
                workspace: { state: 'pending' },
                agent: { state: 'pending' },
              },
            }),
          ),
        post: vi.fn().mockResolvedValue(reply({})),
        fetch: vi.fn(),
      };
      await expect(
        journal.cleanup(
          api as unknown as APIRequestContext,
          'https://example.test',
          'visitor-test',
        ),
      ).rejects.toThrow('quiescence');
      expect(api.fetch).not.toHaveBeenCalled();
      expect(journal.resources.map((row) => row.id)).toContain('created-sandbox');
    },
  );
});
