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
import RedactedReporter from './reporter';
import { fixtureDocument } from './environment.fixture';
import { tc117Fixture, instanceOrigins } from './visitor-abuse-environment';
import type { BrowserContext, Route } from '@playwright/test';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';

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
  it('requires a private manifest before loading campaign identities', () => {
    expect(() =>
      tc117Fixture({ ...fixtureDocument().env, CMA_E2E_FIXTURE_FILE: undefined }),
    ).toThrow('CMA_E2E_FIXTURE_FILE');
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
      'publicJourneys',
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
  it('requires pinned public inputs and dedicated embedding pages for each publisher', () => {
    const fixture = fixtureDocument();
    for (const mutate of [
      (value: typeof fixture.document) => {
        value.publicJourneys.repository.reference = 'main';
      },
      (value: typeof fixture.document) => {
        value.publicJourneys.repository.url = 'https://example.test/private';
      },
      (value: typeof fixture.document) => {
        value.publicJourneys.owners.secondOwner!.embedUrl =
          value.publicJourneys.owners.owner!.embedUrl;
      },
      (value: typeof fixture.document) => {
        value.publicJourneys.owners.owner!.embedUrl = value.embeds.html!.url;
      },
    ]) {
      const value = structuredClone(fixture.document);
      mutate(value);
      writeFileSync(fixture.manifest, JSON.stringify(value));
      expect(() => fixtures(fixture.env)).toThrow();
    }
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
