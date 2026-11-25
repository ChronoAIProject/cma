// @vitest-environment node
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import type { FullConfig, FullResult, TestCase, TestResult } from '@playwright/test/reporter';
import { assertFirstAttempt, selectedVisitors } from './visitor-abuse-preflight';
import TC117Reporter from './visitor-abuse-reporter';
import { newCounts } from './visitor-abuse-model';
import { prepareEvidenceDirectory } from './security';

describe('explicit TC-117 project selection', () => {
  it('excludes TC-117 from the existing default smoke and mutation sets', async () => {
    vi.stubEnv('CMA_E2E_MODE', 'mutation');
    vi.resetModules();
    try {
      const config = (await import('./playwright.config')).default;
      expect(config.projects!.map((project) => project.name)).toEqual(['smoke', 'mutation']);
      expect(config.projects![0]!.testMatch).toBe('**/smoke.pw.ts');
      expect(config.projects![1]!.testIgnore).toContain('**/visitor-abuse*.pw.ts');
      const separate = (await import('./visitor-abuse.config')).default;
      expect(separate.projects!.map((project) => project.name)).toEqual(['tc117-utc', 'tc117']);
      expect(separate.projects!.map((project) => project.testMatch)).toEqual([
        'visitor-abuse-utc.pw.ts',
        'visitor-abuse.pw.ts',
      ]);
    } finally {
      vi.unstubAllEnvs();
    }
  });
  it('rejects retry and repeat overrides before any remote request', () => {
    const config = { projects: [{ retries: 0, repeatEach: 1 }] } as FullConfig;
    expect(() => assertFirstAttempt(config, [])).not.toThrow();
    for (const args of [['--retries', '1'], ['--repeat-each=2']])
      expect(() => assertFirstAttempt(config, args)).toThrow('retry/repetition');
    expect(() =>
      assertFirstAttempt({ projects: [{ retries: 1, repeatEach: 1 }] } as FullConfig, []),
    ).toThrow('retry/repetition');
  });
});

describe('TC-117 redacted campaign summary', () => {
  it.each(['passed', 'skipped', 'failed'] as const)(
    'keeps a %s partial selection distinct from full acceptance',
    (status) => {
      const root = mkdtempSync(join(tmpdir(), 'cma-tc117-reporter-'));
      const output = join(root, 'evidence');
      const run = 'tc117-local-reporter-123456';
      prepareEvidenceDirectory(output, run)();
      vi.stubEnv('CMA_E2E_OUTPUT_DIR', output);
      vi.stubEnv('CMA_E2E_EVIDENCE_RUN', run);
      const stdout = vi.spyOn(process.stdout, 'write').mockReturnValue(true);
      const stderr = vi.spyOn(process.stderr, 'write').mockReturnValue(true);
      try {
        const reporter = new TC117Reporter();
        writeFileSync(
          join(output, 'tc117-shared.json'),
          JSON.stringify({
            schema: 'cma-tc117-scenario-v1',
            campaign: 'TC-117',
            run,
            case: 'shared',
            journalRun: 'journal-shared',
            startedAt: '2026-01-01T00:00:00Z',
            endedAt: '2026-01-01T00:00:01Z',
            N: 1,
            requests: [{}],
            status,
            counts: newCounts(),
          }),
        );
        reporter.onTestEnd(
          { title: 'TC117-shared static case' } as TestCase,
          {
            status,
            retry: 0,
            duration: 1,
            errors: [{ message: 'unpublished-private-diagnostic' }],
          } as TestResult,
        );
        const result = reporter.onEnd({ status: 'passed' } as FullResult);
        const summary = JSON.parse(readFileSync(join(output, 'tc117-summary.json'), 'utf8'));
        expect(summary.fullCampaign).toBe(false);
        expect(summary.status).toBe(status === 'passed' ? 'partial' : 'failed');
        expect(summary.metrics.collateral_error_rate).toBeNull();
        expect(JSON.stringify([stdout.mock.calls, stderr.mock.calls])).not.toContain(
          'unpublished-private-diagnostic',
        );
        if (status !== 'passed') expect(result).toEqual({ status: 'failed' });
      } finally {
        vi.restoreAllMocks();
        vi.unstubAllEnvs();
      }
    },
  );
});

it('validates only explicitly selected project visitors, even when FullConfig contains both projects', () => {
  const config = { projects: [{ name: 'tc117' }, { name: 'tc117-utc' }] } as FullConfig;
  expect(selectedVisitors(config, ['test', '--project=tc117-utc'])).toEqual(['utc']);
  expect(selectedVisitors(config, ['test', '--project', 'tc117'])).toEqual([
    'boundary',
    'a',
    'b',
    'c',
  ]);
  expect(selectedVisitors(config, ['test', '--project=tc117*'])).toHaveLength(5);
  expect(selectedVisitors(config, ['test', '--project', 'tc117', 'tc117-utc'])).toHaveLength(5);
  expect(selectedVisitors(config, ['test', '--project=tc117', '--project=tc117-utc'])).toHaveLength(
    5,
  );
});
