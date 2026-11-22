// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, expect, it, vi } from 'vitest';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';
import RedactedReporter from './reporter';
import { prepareEvidenceDirectory } from './security';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});
for (const mode of ['one', 'invalid', 'duplicate', 'none'])
  it(`reporter retains only a single parseable journey attachment: ${mode}`, () => {
    const directory = mkdtempSync(join(tmpdir(), 'cma-proof-reporter-'));
    const output = join(directory, 'output');
    prepareEvidenceDirectory(output, 'synthetic-proof-run')();
    vi.stubEnv('CMA_E2E_OUTPUT_DIR', output);
    vi.stubEnv('CMA_E2E_EVIDENCE_RUN', 'synthetic-proof-run');
    const write = vi.spyOn(process.stdout, 'write').mockReturnValue(true);
    const attachment = {
      name: 'public-journey-proof',
      contentType: 'application/json',
      body: Buffer.from(mode === 'invalid' ? 'not-json' : '{"run":"synthetic-proof-run"}'),
    };
    try {
      const reporter = new RedactedReporter();
      reporter.onTestEnd(
        { title: 'synthetic-case' } as TestCase,
        {
          status: 'passed',
          duration: 1,
          errors: [{ message: 'PRIVATE_ERROR_SENTINEL' }],
          attachments:
            mode === 'none' ? [] : mode === 'duplicate' ? [attachment, attachment] : [attachment],
        } as unknown as TestResult,
      );
      reporter.onEnd({ status: 'passed' } as FullResult);
      const evidence = readFileSync(join(output, 'results.json'), 'utf8');
      expect(evidence).not.toContain('PRIVATE_ERROR_SENTINEL');
      expect(write.mock.calls.flat().join('')).not.toContain('PRIVATE_ERROR_SENTINEL');
      const result = JSON.parse(evidence).cases[0];
      if (mode === 'none') expect(result).not.toHaveProperty('proof');
      else expect(result.proof).toEqual(mode === 'one' ? { run: 'synthetic-proof-run' } : null);
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
