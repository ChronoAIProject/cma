// @vitest-environment node
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { prepareEvidenceDirectory } from './security';
import { newCounts } from './visitor-abuse-model';
import TC117Reporter from './visitor-abuse-reporter';

const run = 'tc117-synthetic-binding-run';
const names = ['boundary', 'shared', 'load-controls', 'utc'];
let root: string;
let output: string;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), 'cma-tc117-binding-'));
  output = join(root, 'evidence');
  prepareEvidenceDirectory(output, run)();
  vi.stubEnv('CMA_E2E_OUTPUT_DIR', output);
  vi.stubEnv('CMA_E2E_EVIDENCE_RUN', run);
  vi.spyOn(process.stdout, 'write').mockReturnValue(true);
  vi.spyOn(process.stderr, 'write').mockReturnValue(true);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  rmSync(root, { recursive: true, force: true });
});
function evidence(name: string) {
  return {
    schema: 'cma-tc117-scenario-v1',
    campaign: 'TC-117',
    case: name,
    run,
    journalRun: `journal-${name}`,
    status: 'passed',
    startedAt: '2026-01-01T00:00:00.000Z',
    endedAt: '2026-01-01T00:00:01.000Z',
    N: 1,
    counts: newCounts(),
    provenanceVerified: false,
    requests: [{ actor: 'anonymous', instance: 0, method: 'GET', status: 200, ms: 1 }],
  };
}
function finish(mutate?: (row: ReturnType<typeof evidence>) => void) {
  const reporter = new TC117Reporter();
  for (const name of names) {
    const row = evidence(name);
    if (name === 'utc') mutate?.(row);
    writeFileSync(join(output, `tc117-${name}.json`), JSON.stringify(row));
    reporter.onTestEnd(
      { title: `TC117-${name} synthetic scenario` } as TestCase,
      { status: 'passed', retry: 0, duration: 1, errors: [] } as unknown as TestResult,
    );
  }
  expect(reporter.onEnd({ status: 'passed' } as FullResult)).toEqual({ status: 'failed' });
  return JSON.parse(readFileSync(join(output, 'tc117-summary.json'), 'utf8'));
}
it('binds the summary to its run and exact scenario bytes without granting qualification', () => {
  const summary = finish();
  expect(summary.run).toBe(run);
  expect(summary.scenarioStatus).toBe('passed');
  expect(summary.fullCampaign).toBe(false);
  expect(summary.evidence).toEqual(
    names.map((name) => ({
      case: `TC117-${name}`,
      file: `tc117-${name}.json`,
      sha256: createHash('sha256')
        .update(readFileSync(join(output, `tc117-${name}.json`)))
        .digest('hex'),
    })),
  );
});
it.each([
  [
    'wrong run',
    (r: ReturnType<typeof evidence>) => {
      r.run = 'another-synthetic-run';
    },
  ],
  [
    'swapped case',
    (r: ReturnType<typeof evidence>) => {
      r.case = 'shared';
    },
  ],
  [
    'wrong campaign',
    (r: ReturnType<typeof evidence>) => {
      r.campaign = 'another';
    },
  ],
  [
    'missing journal',
    (r: ReturnType<typeof evidence>) => {
      r.journalRun = '';
    },
  ],
  [
    'empty samples',
    (r: ReturnType<typeof evidence>) => {
      r.N = 0;
      r.requests = [];
    },
  ],
  [
    'inconsistent samples',
    (r: ReturnType<typeof evidence>) => {
      r.N = 2;
    },
  ],
  [
    'reversed time',
    (r: ReturnType<typeof evidence>) => {
      r.endedAt = '2025-12-31T00:00:00Z';
    },
  ],
  [
    'legacy unbound evidence',
    (r: ReturnType<typeof evidence>) => {
      r.schema = '';
    },
  ],
] as const)('refuses %s even when the runner reports passed', (_name, mutate) => {
  expect(finish(mutate).scenarioStatus).toBe('failed');
});
