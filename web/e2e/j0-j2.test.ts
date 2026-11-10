// @vitest-environment node
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AgentTranscript } from '@chronoai/cma-sdk';
import type {
  FullConfig,
  FullResult,
  Suite,
  TestCase,
  TestResult,
} from '@playwright/test/reporter';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { assertJourneySelection, journeyResult, requireJourneyMutation } from './j0-j2-policy';
import { assertVisitorSandbox, completedFirstAnswer } from './journey-oracles';
import JourneySliceReporter from './j0-j2-reporter';
import { prepareEvidenceDirectory } from './security';

const temporary: string[] = [];
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  temporary.splice(0).forEach((path) => rmSync(path, { recursive: true, force: true }));
});

describe('J0-J2 execution boundaries', () => {
  const target = 'https://journey.example.test';
  const env = {
    CMA_E2E_BASE_URL: target,
    CMA_E2E_PUBLIC_ID: 'test-trigger',
    CMA_E2E_OUTPUT_DIR: '/tmp/j0-j2-output',
  };
  it('requires explicit mutation mode and the exact approved target', () => {
    expect(() => requireJourneyMutation(env)).toThrow('explicit mutation');
    expect(() => requireJourneyMutation({ ...env, CMA_E2E_MODE: 'smoke' })).toThrow();
    expect(() => requireJourneyMutation({ ...env, CMA_E2E_MODE: 'mutation' })).toThrow(
      'exact base origin',
    );
    expect(() =>
      requireJourneyMutation({
        ...env,
        CMA_E2E_MODE: 'mutation',
        CMA_E2E_MUTATION_TARGET: 'https://other.example.test',
      }),
    ).toThrow('exact base origin');
    expect(
      requireJourneyMutation({
        ...env,
        CMA_E2E_MODE: 'mutation',
        CMA_E2E_MUTATION_TARGET: target,
      }).mode,
    ).toBe('mutation');
  });

  it('rejects omitted, duplicated, full-campaign and owner-only selections', () => {
    expect(() => assertJourneySelection(['J0 signup', 'J1 fresh', 'J2 workspace'])).not.toThrow();
    for (const titles of [
      [],
      ['J0 signup'],
      ['J0 signup', 'J1 fresh', 'J1 again'],
      ['J0 signup', 'J1 fresh', 'J2 workspace', 'J7 paused'],
      ['J0 signup', 'J1 fresh', 'OWNER existing'],
    ])
      expect(() => assertJourneySelection(titles)).toThrow('exactly one case');
  });

  const rows = ['J0 signup', 'J1 fresh', 'J2 workspace'].map((name) => ({
    case: name,
    status: 'passed',
    retry: 0,
    durationMs: 1,
  }));
  it('records successful execution as partial evidence without advancing the gate', () => {
    expect(journeyResult('passed', rows)).toMatchObject({
      status: 'passed',
      attempted: 3,
      passed: 3,
      scope: 'j0_j2_partial',
      fullCampaignGatesAdvanced: false,
      samplingTargetsSatisfied: false,
      deferredJourneys: ['J3', 'J4', 'J5', 'J6', 'J7'],
    });
  });
  it('does not count skipped, retried, cancelled or incomplete runs as a passing slice', () => {
    for (const status of ['skipped', 'failed', 'timedOut', 'interrupted']) {
      const result = journeyResult('passed', [rows[0]!, rows[1]!, { ...rows[2]!, status }]);
      expect(result.status).toBe('failed');
      expect(result.passed).toBe(2);
      expect(result.attempted).toBe(status === 'skipped' ? 2 : 3);
    }
    expect(journeyResult('passed', [rows[0]!, rows[1]!, { ...rows[2]!, retry: 1 }]).status).toBe(
      'failed',
    );
    expect(journeyResult('interrupted', rows).status).toBe('failed');
    expect(journeyResult('passed', rows.slice(0, 2)).status).toBe('failed');
  });

  it('fails the reporter result even when an invalid configuration produces three passing tests', () => {
    vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    const reporter = new JourneySliceReporter();
    reporter.onBegin(
      { projects: [{ retries: 1, repeatEach: 1 }] } as FullConfig,
      { allTests: () => rows.map((row) => ({ title: row.case })) } as Suite,
    );
    for (const row of rows)
      reporter.onTestEnd(
        { title: row.case } as TestCase,
        { status: 'passed', retry: 0, duration: 1 } as TestResult,
      );
    expect(reporter.onEnd({ status: 'passed' } as FullResult)).toEqual({ status: 'failed' });
  });

  it('writes only a partial report after validated setup, without raw errors or full results', () => {
    const parent = mkdtempSync(join(tmpdir(), 'cma-j0-j2-test-'));
    temporary.push(parent);
    const output = join(parent, 'evidence');
    const run = 'j0-j2-private-run';
    const complete = prepareEvidenceDirectory(output, run);
    vi.stubEnv('CMA_E2E_OUTPUT_DIR', output);
    vi.stubEnv('CMA_E2E_EVIDENCE_RUN', run);
    vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    const reporter = new JourneySliceReporter();
    for (const row of rows)
      reporter.onTestEnd(
        { title: row.case } as TestCase,
        {
          status: row.status,
          retry: row.retry,
          duration: 1,
          errors: [{ message: 'PRIVATE-DIAGNOSTIC' }],
        } as TestResult,
      );
    const result = { status: 'passed' } as FullResult;
    reporter.onEnd(result);
    expect(readdirSync(output)).toEqual([]);
    complete();
    expect(reporter.onEnd(result)).toEqual({ status: 'passed' });
    expect(readdirSync(output).sort()).toEqual(['.validated-run', 'j0-j2-results.json']);
    const text = readFileSync(join(output, 'j0-j2-results.json'), 'utf8');
    expect(text).not.toContain('PRIVATE-DIAGNOSTIC');
    expect(JSON.parse(text)).toMatchObject({
      scope: 'j0_j2_partial',
      fullCampaignGatesAdvanced: false,
    });
  });
});

describe('new-user journey independent observations', () => {
  const transcript: AgentTranscript = {
    agent_id: 'test-agent',
    through_seq: 3,
    turns: [
      {
        turn_sequence: 1,
        started_at: '2026-11-11T00:00:00Z',
        owner_message: 'Reply briefly',
        agent_messages: ['Ready'],
        messages: [
          { sequence: 2, role: 'assistant', content: 'Ready', appended_at: '2026-11-11T00:00:01Z' },
        ],
        outcome: { type: 'completed', at: '2026-11-11T00:00:02Z' },
      },
    ],
  };
  it('requires the actual visitor-owned Sandbox rather than the publisher or a different resource', () => {
    expect(() =>
      assertVisitorSandbox(
        { id: 'sandbox-test', owner: 'visitor-test' },
        'sandbox-test',
        'visitor-test',
      ),
    ).not.toThrow();
    for (const value of [
      { id: 'sandbox-test', owner: 'publisher-test' },
      { id: 'different', owner: 'visitor-test' },
      {},
    ])
      expect(() => assertVisitorSandbox(value, 'sandbox-test', 'visitor-test')).toThrow(
        'authenticated visitor',
      );
    expect(() => assertVisitorSandbox({ id: '', owner: '' }, '', '')).toThrow();
  });
  it('requires exactly one completed turn with a nonempty assistant answer for the admitted Agent', () => {
    expect(completedFirstAnswer(transcript, 'test-agent')).toBe(true);
    expect(completedFirstAnswer(transcript, 'other-agent')).toBe(false);
    expect(completedFirstAnswer({ ...transcript, turns: [] }, 'test-agent')).toBe(false);
    expect(
      completedFirstAnswer(
        { ...transcript, turns: [...transcript.turns, ...transcript.turns] },
        'test-agent',
      ),
    ).toBe(false);
    const turn = transcript.turns[0]!;
    for (const changed of [
      { ...turn, outcome: { type: 'active' as const } },
      { ...turn, messages: [] },
      { ...turn, messages: [{ ...turn.messages[0]!, content: '  ' }] },
      { ...turn, messages: [{ ...turn.messages[0]!, role: 'user' as const }] },
    ])
      expect(completedFirstAnswer({ ...transcript, turns: [changed] }, 'test-agent')).toBe(false);
  });
});
