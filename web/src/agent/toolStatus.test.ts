import { describe, expect, it } from 'vitest';
import { isPendingTool, toolOutcome, toolOutcomeWord } from './toolStatus';

describe('toolOutcome', () => {
  it.each([
    ['completed', 'completed'],
    ['Succeeded', 'completed'],
    ['failed', 'failed'],
    ['ERROR', 'failed'],
    ['cancelled', 'cancelled'],
    ['interrupted', 'cancelled'],
  ] as const)('reads the settled status %s as %s', (status, outcome) => {
    expect(toolOutcome({ status, runId: '1' }, '1')).toBe(outcome);
  });

  it('calls a pending call in the active turn running', () => {
    expect(toolOutcome({ status: 'in_progress', runId: '2' }, '2')).toBe('running');
  });

  it('never claims a pending call from an ended or other turn is still running', () => {
    expect(toolOutcome({ status: 'in_progress', runId: '1' }, undefined)).toBe('unreported');
    expect(toolOutcome({ status: 'pending', runId: '1' }, '2')).toBe('unreported');
    expect(toolOutcome({ status: 'mystery', runId: undefined }, '2')).toBe('unreported');
  });
});

describe('toolOutcomeWord', () => {
  it('uses plain words and keeps the last status only as evidence', () => {
    expect(toolOutcomeWord('running', 'in_progress')).toBe('In progress');
    expect(toolOutcomeWord('completed', 'success')).toBe('Completed');
    expect(toolOutcomeWord('failed', 'error')).toBe('Failed');
    expect(toolOutcomeWord('cancelled', 'stopped')).toBe('Cancelled');
    expect(toolOutcomeWord('unreported', 'in_progress')).toBe(
      'Outcome not reported (last status: in_progress)',
    );
  });
});

describe('isPendingTool', () => {
  it('treats only recognised settled states as settled', () => {
    expect(isPendingTool({ status: 'Completed' })).toBe(false);
    expect(isPendingTool({ status: 'aborted' })).toBe(false);
    expect(isPendingTool({ status: 'queued' })).toBe(true);
  });
});
