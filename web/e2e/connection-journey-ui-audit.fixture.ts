import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { TestInfo } from '@playwright/test';
import type { TestCase, TestResult } from '@playwright/test/reporter';
import type { Harness } from './harness';
import RedactedReporter from './reporter';
import { prepareEvidenceDirectory } from './security';
import { connectionCaseTitles } from '../../.github/scripts/connection-journey-evidence.mjs';

export async function auditWizardProof(h: Harness, info: TestInfo) {
  await h.flush();
  await h.attachJourneyProof(info);
  assert.equal(info.attachments.length, 1);
  assert.equal(info.attachments[0]!.name, 'connection-journey-proof');
  const reporter = new RedactedReporter();
  reporter.onTestEnd(
    { title: connectionCaseTitles[h.caseId as keyof typeof connectionCaseTitles] } as TestCase,
    { status: 'passed', duration: 1, attachments: info.attachments } as TestResult,
  );
  const output = join(h.config.output, 'reporter');
  prepareEvidenceDirectory(output, h.journal.run)();
  const previous = [process.env.CMA_E2E_OUTPUT_DIR, process.env.CMA_E2E_EVIDENCE_RUN];
  process.env.CMA_E2E_OUTPUT_DIR = output;
  process.env.CMA_E2E_EVIDENCE_RUN = h.journal.run;
  try {
    reporter.onEnd({ status: 'passed' } as Parameters<RedactedReporter['onEnd']>[0]);
  } finally {
    for (const [index, key] of ['CMA_E2E_OUTPUT_DIR', 'CMA_E2E_EVIDENCE_RUN'].entries()) {
      if (previous[index] === undefined) delete process.env[key];
      else process.env[key] = previous[index];
    }
  }
  const path = join(output, 'results.json');
  assert.equal(statSync(path).mode & 0o777, 0o600);
  const results = JSON.parse(readFileSync(path, 'utf8'));
  assert.deepEqual(
    results.cases[0].connectionProof.commands,
    JSON.parse(JSON.stringify(h.commands)),
  );
  const journal = readFileSync(join(h.config.output, 'cleanup.ndjson'), 'utf8')
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line));
  execFileSync(
    process.execPath,
    [fileURLToPath(new URL('./connection-journey-ui-audit.fixture.mjs', import.meta.url))],
    {
      cwd: fileURLToPath(new URL('../..', import.meta.url)),
      input: JSON.stringify({
        results,
        journal,
        fixture: {
          visitor: h.fixture.accounts.visitor,
          connectionJourneys: h.fixture.connectionJourneys,
        },
      }),
      timeout: 10_000,
      stdio: ['pipe', 'pipe', 'pipe'],
    },
  );
}
