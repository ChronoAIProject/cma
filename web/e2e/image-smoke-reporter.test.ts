import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { FullConfig, Suite, TestCase, TestResult, TestStep } from '@playwright/test/reporter';
import { afterEach, expect, it } from 'vitest';
import ImageSmokeReporter from './image-smoke-reporter';

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true });
});

function harness() {
  const directory = mkdtempSync(join(tmpdir(), 'image-reporter-'));
  directories.push(directory);
  const reporter = new ImageSmokeReporter();
  reporter.onBegin(
    {
      projects: [{ outputDir: directory }],
      rootDir: '/repo/e2e',
      workers: 1,
      globalTimeout: 240_000,
    } as FullConfig,
    { allTests: () => [test] } as Suite,
  );
  const events = () =>
    readFileSync(join(directory, 'browser-events.jsonl'), 'utf8')
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
  return { reporter, events };
}

const test = {
  id: 'case-1',
  titlePath: () => ['chromium', 'image.check.ts', 'case'],
  timeout: 60_000,
} as TestCase;
const step = {
  category: 'pw:api',
  title: 'private input must not be recorded',
  location: { file: '/repo/e2e/image.check.ts', line: 12, column: 4 },
  duration: 125,
} as TestStep;

it('persists the active test and operation before teardown or any end event', () => {
  const { reporter, events } = harness();
  reporter.onTestBegin(test);
  reporter.onStepBegin(test, {} as TestResult, step);
  expect(events()).toEqual([
    expect.objectContaining({ event: 'suite-begin', tests: 1, globalTimeoutMs: 240_000 }),
    expect.objectContaining({ event: 'test-begin', testId: 'case-1', timeoutMs: 60_000 }),
    expect.objectContaining({
      event: 'step-begin',
      testId: 'case-1',
      location: { file: 'image.check.ts', line: 12, column: 4 },
    }),
  ]);
  expect(JSON.stringify(events())).not.toContain('private input');
});

for (const status of ['passed', 'timedOut', 'interrupted'] as const) {
  it(`retains timings and ${status} status`, () => {
    const { reporter, events } = harness();
    reporter.onTestBegin(test);
    reporter.onStepEnd(test, {} as TestResult, {
      ...step,
      error: status === 'passed' ? undefined : { message: 'private failure details' },
    });
    reporter.onTestEnd(test, { status, duration: 150 } as TestResult);
    reporter.onEnd({
      status: status === 'passed' ? 'passed' : 'failed',
      duration: 200,
      startTime: new Date(),
    });
    expect(events().at(-3)).toMatchObject({
      event: 'step-end',
      durationMs: 125,
      failed: status !== 'passed',
    });
    expect(events().at(-2)).toMatchObject({ event: 'test-end', status, durationMs: 150 });
    expect(events().at(-1)).toMatchObject({ event: 'suite-end', durationMs: 200 });
    expect(JSON.stringify(events())).not.toContain('private failure details');
  });
}
