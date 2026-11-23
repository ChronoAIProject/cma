import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  FullConfig,
  FullResult,
  Reporter,
  Suite,
  TestCase,
  TestError,
  TestResult,
} from '@playwright/test/reporter';
import { outsideRepository } from './environment';
import { evidenceDirectoryIsReady } from './security';

export default class RedactedReporter implements Reporter {
  private rows: {
    case: string;
    status: string;
    durationMs: number;
    proof?: unknown;
    connectionProof?: unknown;
  }[] = [];
  onBegin(_config: FullConfig, suite: Suite) {
    process.stdout.write(`Discovered ${suite.allTests().length} Playwright cases.\n`);
    for (const test of suite.allTests()) process.stdout.write(`CASE ${test.title}\n`);
  }
  onTestEnd(test: TestCase, result: TestResult) {
    // Assertion errors can contain DOM, response bodies and identity-provider URLs.
    const row: (typeof this.rows)[number] = {
      case: test.title,
      status: result.status,
      durationMs: result.duration,
    };
    for (const [name, field] of [
      ['public-journey-proof', 'proof'],
      ['connection-journey-proof', 'connectionProof'],
    ] as const) {
      const proofs = (result.attachments ?? []).filter((item) => item.name === name);
      if (proofs.length > 0) {
        row[field] = null;
        if (
          proofs.length === 1 &&
          proofs[0]?.body &&
          proofs[0].contentType === 'application/json'
        ) {
          try {
            row[field] = JSON.parse(proofs[0].body.toString());
          } catch {
            /* Invalid proof remains null and final audit refuses it. */
          }
        }
      }
    }
    this.rows.push(row);
    process.stdout.write(`${row.status}: ${row.case}\n`);
  }
  onError(error: TestError) {
    const message = error.message?.split('\n')[0]?.replace(/^Error: /, '') ?? '';
    if (
      /^(Required E2E|Invalid E2E|E2E |Mutation mode|CMA_E2E_|Fixture origin|Each owner)/.test(
        message,
      )
    ) {
      process.stderr.write(`${message}\n`);
      return;
    }
    process.stderr.write(
      'E2E prerequisite or runner failure. Inspect the private local diagnostics; no raw values are published.\n',
    );
  }
  onEnd(result: FullResult) {
    if (this.rows.length === 0) {
      process.stdout.write(
        `Harness discovery or prerequisite status: ${result.status}; no journeys executed.\n`,
      );
      return;
    }
    process.stdout.write(
      `Staging harness status: ${result.status}; executed cases: ${this.rows.length}.\n`,
    );
    if (!process.env.CMA_E2E_OUTPUT_DIR) return;
    const output = outsideRepository(process.env.CMA_E2E_OUTPUT_DIR, 'CMA_E2E_OUTPUT_DIR');
    if (!evidenceDirectoryIsReady(output, process.env.CMA_E2E_EVIDENCE_RUN)) return;
    writeFileSync(
      join(output, 'results.json'),
      JSON.stringify({ status: result.status, cases: this.rows }),
      { mode: 0o600, flag: 'wx' },
    );
  }
}
