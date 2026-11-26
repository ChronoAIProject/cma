// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { prepareEvidenceDirectory } from './security';
import { newCounts } from './visitor-abuse-model';
import TC117Reporter from './visitor-abuse-reporter';

const names = ['boundary', 'shared', 'load-controls', 'utc'];
let root: string;
let output: string;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), 'cma-tc117-summary-'));
  output = join(root, 'evidence');
  const run = 'tc117-synthetic-reporter-run';
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

function record(
  reporter: TC117Reporter,
  name: string,
  status: TestResult['status'] = 'passed',
  retry = 0,
  evidence: unknown = { status, counts: newCounts(), provenanceVerified: false },
) {
  writeFileSync(
    join(output, `tc117-${name}.json`),
    JSON.stringify({
      schema: 'cma-tc117-scenario-v1',
      campaign: 'TC-117',
      case: name,
      run: process.env.CMA_E2E_EVIDENCE_RUN,
      journalRun: `journal-${name}`,
      startedAt: '2026-01-01T00:00:00Z',
      endedAt: '2026-01-01T00:00:01Z',
      N: 1,
      requests: [{}],
      ...(evidence as object),
    }),
  );
  reporter.onTestEnd(
    { title: `TC117-${name} synthetic scenario` } as TestCase,
    { status, retry, duration: 1, errors: [] } as unknown as TestResult,
  );
}
function finish(reporter: TC117Reporter, status: FullResult['status'] = 'passed') {
  const result = reporter.onEnd({ status } as FullResult);
  const summary = JSON.parse(readFileSync(join(output, 'tc117-summary.json'), 'utf8'));
  const generic = JSON.parse(readFileSync(join(output, 'results.json'), 'utf8'));
  expect(result).toEqual({ status: 'failed' });
  expect(generic.status).toBe('failed');
  expect(summary.fullCampaign).toBe(false);
  expect(summary.provenanceVerified).toBe(false);
  expect(summary.status).not.toBe('passed');
  return summary;
}

describe('TC-117 qualification cannot be inferred from scenario outcomes', () => {
  it('blocks full acceptance even when all four scenarios pass', () => {
    const reporter = new TC117Reporter();
    for (const name of names) record(reporter, name);
    const summary = finish(reporter);
    expect(summary.status).toBe('blocked');
    expect(summary.scenarioStatus).toBe('passed');
    expect(summary.metrics.collateral_error_rate).toBeNull();
  });
  it('does not accept a self-asserted provenance flag', () => {
    const reporter = new TC117Reporter();
    for (const name of names)
      record(reporter, name, 'passed', 0, {
        status: 'passed',
        counts: newCounts(),
        provenanceVerified: true,
      });
    expect(finish(reporter).status).toBe('blocked');
  });
  it.each(['failed', 'timedout', 'interrupted'] as const)(
    'keeps an overall %s result failed even with four passing case records',
    (status) => {
      const reporter = new TC117Reporter();
      for (const name of names) record(reporter, name);
      expect(finish(reporter, status).status).toBe('failed');
    },
  );
  it.each(['failed', 'timedOut', 'interrupted', 'skipped'] as const)(
    'rejects a %s case despite a passed runner result',
    (status) => {
      const reporter = new TC117Reporter();
      for (const name of names) record(reporter, name, name === 'utc' ? status : 'passed');
      expect(finish(reporter).status).toBe('failed');
    },
  );
  it('keeps partial selection unsuccessful', () => {
    const reporter = new TC117Reporter();
    record(reporter, 'shared');
    expect(finish(reporter).status).toBe('partial');
  });
  it.each(['retry', 'repeat', 'unknown', 'missing', 'blocked', 'malformed'])(
    'rejects %s evidence',
    (kind) => {
      const reporter = new TC117Reporter();
      for (const name of names) record(reporter, name, 'passed', kind === 'retry' ? 1 : 0);
      if (kind === 'repeat') record(reporter, 'shared');
      if (kind === 'unknown') record(reporter, 'extra');
      if (kind === 'missing') rmSync(join(output, 'tc117-utc.json'));
      if (kind === 'blocked' || kind === 'malformed')
        writeFileSync(
          join(output, 'tc117-utc.json'),
          JSON.stringify({
            status: kind === 'blocked' ? 'blocked' : 'passed',
            counts: kind === 'malformed' ? { offered: -1 } : newCounts(),
          }),
        );
      expect(finish(reporter).status).toBe('failed');
    },
  );
  it('fails executed cases without a validated evidence directory', () => {
    const reporter = new TC117Reporter();
    record(reporter, 'shared');
    vi.stubEnv('CMA_E2E_EVIDENCE_RUN', 'wrong-run');
    expect(reporter.onEnd({ status: 'passed' } as FullResult)).toEqual({ status: 'failed' });
  });
  it('allows discovery without claiming that any scenario executed', () => {
    const reporter = new TC117Reporter();
    expect(reporter.onEnd({ status: 'passed' } as FullResult)).toBeUndefined();
    expect(process.stdout.write).toHaveBeenCalledWith(
      expect.stringContaining('no journeys executed'),
    );
  });
});
