import { render, screen } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
import { Thinking } from './Thinking';
import { ActiveThinking } from './ActiveThinking';
import { conversationFromTranscript } from './conversation';
import { expect, it } from 'vitest';
import { elapsedTime, toolDuration, durationLabel } from './workspaceTime';
import { runningTaskTime } from '../shell/taskGroups';
it.each([
  [-1000, '0s'],
  [59_999, '59s'],
  [60_000, '1m 0s'],
  [3_599_999, '59m 59s'],
  [3_600_000, '1h 0m'],
  [3_900_000, '1h 5m'],
  [86_399_999, '23h 59m'],
  [86_400_000, '1d 0h'],
  [183_600_000, '2d 3h'],
  [5_342_119_000, '61d 19h'],
])('formats %i milliseconds without unbounded minute counts', (duration, expected) => {
  expect(elapsedTime(duration)).toBe(expected);
});
it('uses the same long-run units in sidebar badges while preserving short-run labels', () => {
  const start = '2026-11-06T00:00:00Z';
  const at = Date.parse(start);
  expect(runningTaskTime(start, new Date(at + 59_999))).toBe('<1m');
  expect(runningTaskTime(start, new Date(at + 180_000))).toBe('3m');
  expect(runningTaskTime(start, new Date(at + 3_900_000))).toBe('1h 5m');
  expect(runningTaskTime(start, new Date(at + 183_600_000))).toBe('2d 3h');
});

afterEach(() => vi.useRealTimers());
it('renders compact hours in Thinking and days in the interrupted response status', () => {
  vi.useFakeTimers();
  const start = '2026-11-06T00:00:00Z';
  vi.setSystemTime(Date.parse(start) + 3_900_000);
  const view = render(<Thinking startedAt={start} />);
  expect(screen.getByText('1h 5m')).toBeVisible();
  const finished = new Date(Date.parse(start) + 183_600_000).toISOString();
  view.rerender(
    <ActiveThinking
      conversation={conversationFromTranscript({
        agentId: 'agent',
        throughSeq: 2,
        turns: [
          {
            turnSequence: 1,
            ownerMessage: '',
            agentMessages: [],
            messages: [],
            startedAt: start,
            outcome: { type: 'interrupted', at: finished },
          },
        ],
      })}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent(
    'Response interrupted after 2d 3h. Running Sandbox commands may continue.',
  );
});

it.each([
  [undefined, '2026-11-05T14:33:00Z'],
  ['2026-11-05T14:33:00Z', undefined],
  ['invalid', '2026-11-05T14:33:00Z'],
  ['2026-11-05T14:33:00Z', 'invalid'],
  ['2026-11-05T14:33:00Z', '2026-11-05T14:33:00Z'],
  ['2026-11-05T14:33:01Z', '2026-11-05T14:33:00Z'],
])('does not infer tool time from unusable endpoints %s -> %s', (start, end) => {
  expect(toolDuration(start, end)).toBeUndefined();
});

it.each([
  [20, '<0.1s'],
  [99, '<0.1s'],
  [100, '0.1s'],
  [550, '0.6s'],
])('preserves positive recorded time without rounding %i ms to zero', (value, label) => {
  expect(durationLabel(value)).toBe(label);
});
