import { expect, it } from 'vitest';
import type { ToolCall } from './model';
import { summarizeTools } from './toolSummary';

const start = '2026-11-05T14:33:00.000Z';
const finish = '2026-11-05T14:33:00.550Z';
const call = (extra: Partial<ToolCall> = {}): ToolCall => ({
  id: 'synthetic-tool',
  kind: 'execute',
  title: 'Check fixture',
  status: 'completed',
  startedAt: start,
  finishedAt: finish,
  ...extra,
});

it.each(['execute', 'read', 'search', 'edit', 'other'])(
  'marks same-instant %s timing unavailable',
  (kind) => {
    expect(summarizeTools([call({ kind, finishedAt: start })]).duration).toBe(
      'Duration unavailable',
    );
  },
);

it.each([
  { startedAt: undefined },
  { finishedAt: undefined },
  { startedAt: 'invalid' },
  { finishedAt: 'invalid' },
  { startedAt: finish, finishedAt: start },
])('does not add a missing or invalid interval as zero: %j', (extra) => {
  expect(summarizeTools([call(), call({ id: 'second', ...extra })]).duration).toBe(
    'Duration unavailable',
  );
});

it('keeps the sum of parallel tool intervals, including mixed tool kinds', () => {
  expect(
    summarizeTools([
      call(),
      call({ id: 'read', kind: 'read' }),
      call({ id: 'search', kind: 'search' }),
    ]),
  ).toMatchObject({
    label: 'Ran 1 command · Read 1 file · Searched 1 time',
    duration: '1.7s',
  });
});

it('does not imply fractional precision for whole-second timestamps, including mixed precision groups', () => {
  const seconds = call({ startedAt: '2026-11-05T14:33:00Z', finishedAt: '2026-11-05T14:33:02Z' });
  expect(summarizeTools([seconds]).duration).toBe('2s');
  expect(summarizeTools([seconds, call()]).duration).toBe('3s');
});

it.each(['running', 'in_progress', 'future-status'])(
  'does not publish a final duration for %s tools',
  (status) => {
    expect(summarizeTools([call(), call({ status })]).duration).toBeUndefined();
  },
);

it.each(['cma-browser navigate https://example.test', 'xdotool click 1'])(
  'keeps screen interval semantics and validates every step: %s',
  (title) => {
    const first = call({ title });
    const last = call({
      title,
      id: 'last',
      startedAt: '2026-11-05T14:33:02.000Z',
      finishedAt: '2026-11-05T14:33:03.000Z',
    });
    expect(summarizeTools([first, last]).duration).toBe('3s');
    for (const extra of [
      { startedAt: undefined },
      { finishedAt: undefined },
      { finishedAt: start },
    ])
      expect(summarizeTools([first, call({ title, id: 'middle', ...extra }), last]).duration).toBe(
        'Duration unavailable',
      );
    expect(summarizeTools([first, { ...last, status: 'running' }]).duration).toBeUndefined();
  },
);
