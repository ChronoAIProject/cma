import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import type {
  FullConfig,
  FullResult,
  Reporter,
  Suite,
  TestCase,
  TestResult,
  TestStep,
} from '@playwright/test/reporter';

/** Write through each event: a hard-killed runner cannot flush an end-only report. */
export default class ImageSmokeReporter implements Reporter {
  private path = '';
  private root = '';

  onBegin(config: FullConfig, suite: Suite) {
    const directory = config.projects[0]!.outputDir;
    mkdirSync(directory, { recursive: true });
    this.path = join(directory, 'browser-events.jsonl');
    this.root = config.rootDir;
    writeFileSync(this.path, '');
    this.record('suite-begin', {
      tests: suite.allTests().length,
      workers: config.workers,
      globalTimeoutMs: config.globalTimeout,
    });
  }

  onTestBegin(test: TestCase) {
    this.record('test-begin', {
      testId: test.id,
      title: test.titlePath(),
      timeoutMs: test.timeout,
    });
  }

  onStepBegin(test: TestCase, _result: TestResult, step: TestStep) {
    this.step('step-begin', test, step);
  }

  onStepEnd(test: TestCase, _result: TestResult, step: TestStep) {
    this.step('step-end', test, step);
  }

  private step(event: string, test: TestCase, step: TestStep) {
    // Locations identify the blocked operation without recording inputs or page contents.
    this.record(event, {
      testId: test.id,
      category: step.category,
      location: step.location && {
        ...step.location,
        file: relative(this.root, step.location.file),
      },
      ...(event === 'step-end' ? { durationMs: step.duration, failed: !!step.error } : {}),
    });
  }

  onTestEnd(test: TestCase, result: TestResult) {
    this.record('test-end', {
      testId: test.id,
      status: result.status,
      durationMs: result.duration,
    });
  }

  onEnd(result: FullResult) {
    this.record('suite-end', { status: result.status, durationMs: result.duration });
  }

  private record(event: string, fields: Record<string, unknown>) {
    appendFileSync(
      this.path,
      `${JSON.stringify({ event, at: new Date().toISOString(), ...fields })}\n`,
    );
  }
}
