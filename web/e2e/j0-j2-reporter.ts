import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  FullConfig,
  FullResult,
  Suite,
  TestCase,
  TestResult,
} from '@playwright/test/reporter';
import { outsideRepository } from './environment';
import { assertJourneySelection, journeyResult, type JourneyResult } from './j0-j2-policy';
import RedactedReporter from './reporter';
import { evidenceDirectoryIsReady } from './security';

export default class JourneySliceReporter extends RedactedReporter {
  private cases: JourneyResult[] = [];
  private invalidSelection = false;

  onBegin(config: FullConfig, suite: Suite) {
    // Prerequisite or discovery failures can reach onBegin with an empty suite.
    if (suite.allTests().length === 0) return;
    try {
      assertJourneySelection(suite.allTests().map((test) => test.title));
      if (config.projects.some((project) => project.retries !== 0 || project.repeatEach !== 1))
        throw new Error('Invalid E2E J0-J2 retry/repetition configuration');
    } catch {
      // Playwright swallows reporter exceptions; retain invalidity through onEnd.
      this.invalidSelection = true;
      process.stderr.write('Invalid E2E J0-J2 selection or retry/repetition configuration.\n');
    }
    super.onBegin(config, suite);
    process.stdout.write('J0-J2 partial execution; full campaign and sampling gates unchanged.\n');
  }

  onTestEnd(test: TestCase, result: TestResult) {
    this.cases.push({
      case: test.title,
      status: result.status,
      retry: result.retry,
      durationMs: result.duration,
    });
    process.stdout.write(`${result.status}: ${test.title}\n`);
  }

  onEnd(result: FullResult) {
    if (this.cases.length === 0) {
      process.stdout.write(
        `J0-J2 discovery/prerequisite status: ${result.status}; no journeys executed.\n`,
      );
      return this.invalidSelection ? { status: 'failed' as const } : undefined;
    }
    const record = journeyResult(this.invalidSelection ? 'failed' : result.status, this.cases);
    process.stdout.write(
      `J0-J2 partial status: ${record.status}; full campaign gates unchanged.\n`,
    );
    const path = process.env.CMA_E2E_OUTPUT_DIR;
    if (path && evidenceDirectoryIsReady(path, process.env.CMA_E2E_EVIDENCE_RUN))
      writeFileSync(
        join(outsideRepository(path, 'CMA_E2E_OUTPUT_DIR'), 'j0-j2-results.json'),
        JSON.stringify(record),
        { mode: 0o600, flag: 'wx' },
      );
    return { status: record.status as 'passed' | 'failed' };
  }
}
