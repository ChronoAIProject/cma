import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  FullConfig,
  FullResult,
  Suite,
  TestCase,
  TestResult,
} from '@playwright/test/reporter';
import RedactedReporter from './reporter';
import { evidenceDirectoryIsReady } from './security';
import { outsideRepository } from './environment';
import { assertOwnerCase, ownerScope } from './owner-environment';

export default class OwnerReporter extends RedactedReporter {
  private ownerRows: { case: string; status: string; durationMs: number }[] = [];
  onBegin(_config: FullConfig, suite: Suite) {
    for (const test of suite.allTests()) {
      assertOwnerCase(test.title);
      process.stdout.write(`CASE ${test.title}\n`);
    }
    process.stdout.write(
      `Owner-only partial discovery: ${suite.allTests().length} cases; full campaign gates unchanged.\n`,
    );
  }
  onTestEnd(test: TestCase, result: TestResult) {
    assertOwnerCase(test.title);
    const row = { case: test.title, status: result.status, durationMs: result.duration };
    this.ownerRows.push(row);
    process.stdout.write(`${row.status}: ${row.case}\n`);
  }
  onEnd(result: FullResult) {
    if (this.ownerRows.length === 0) {
      process.stdout.write(
        `Owner-only discovery or prerequisite status: ${result.status}; no remote scenarios executed.\n`,
      );
      return;
    }
    process.stdout.write(
      `Owner-only remote partial status: ${result.status}; executed cases: ${this.ownerRows.length}; full campaign gates unchanged.\n`,
    );
    if (!process.env.CMA_OWNER_E2E_OUTPUT_DIR) return;
    const output = outsideRepository(
      process.env.CMA_OWNER_E2E_OUTPUT_DIR,
      'CMA_OWNER_E2E_OUTPUT_DIR',
    );
    if (!evidenceDirectoryIsReady(output, process.env.CMA_OWNER_E2E_RUN)) return;
    writeFileSync(
      join(output, 'owner-results.json'),
      JSON.stringify({
        ...ownerScope,
        status: result.status,
        cases: this.ownerRows,
      }),
      { mode: 0o600, flag: 'wx' },
    );
  }
}
