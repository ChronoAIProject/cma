import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { taskGroup, taskTime, runningTaskTime, nextTaskTick, taskColor } from './taskGroups';
import { useTaskClock } from './useTaskClock';
import { readFileSync } from 'node:fs';
import { resolveToken, themes } from '../test/palette';
afterEach(() => vi.useRealTimers());
it('ticks only when a running label changes or a local date group rolls over', () => {
  vi.useFakeTimers();
  const start = new Date(2026, 10, 6, 12, 0, 15).getTime();
  vi.setSystemTime(start);
  const { result } = renderHook(() => useTaskClock([new Date(start).toISOString()]));
  const first = result.current;
  expect(runningTaskTime(new Date(start).toISOString(), first)).toBe('<1m');
  act(() => vi.advanceTimersByTime(59_999));
  expect(result.current).toBe(first);
  act(() => vi.advanceTimersByTime(1));
  expect(runningTaskTime(new Date(start).toISOString(), result.current)).toBe('1m');
  const midnight = new Date(2026, 10, 7).getTime();
  expect(nextTaskTick([], midnight - 1)).toBe(1);
  expect(taskGroup('completed', new Date(start).toISOString(), new Date(midnight))).toBe(
    'Yesterday',
  );
  expect(taskTime(new Date(start).toISOString(), first)).toBe('12:00');
  expect(taskTime(new Date(start).toISOString(), new Date(midnight))).toBe('Nov 6');
  vi.setSystemTime(start + 180_000);
  act(() => window.dispatchEvent(new Event('focus')));
  expect(runningTaskTime(new Date(start).toISOString(), result.current)).toBe('3m');
});
it('uses stable theme-aware fallback colours with at least 3:1 sidebar contrast', () => {
  const css = readFileSync('src/styles/tokens.css', 'utf8');
  const palette = themes();
  const values = (suffix: string) =>
    [...css.matchAll(new RegExp(`--dsw-alias-workspace-${suffix}: (#[a-f0-9]+);`, 'g'))].map(
      (x) => x[1]!,
    );
  const light = (hex: string) => {
    const rgb = hex
      .slice(1)
      .match(/../g)!
      .map((n) => parseInt(n, 16) / 255)
      .map((n) => (n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4));
    return 0.2126 * rgb[0]! + 0.7152 * rgb[1]! + 0.0722 * rgb[2]!;
  };
  for (const role of ['task-blue', 'task-purple', 'accent']) {
    values(role).forEach((ink, theme) => {
      const a = light(ink),
        b = light(
          resolveToken('dsw-specific-sidebar-fill', theme === 0 ? palette.light : palette.dark),
        );
      expect((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)).toBeGreaterThanOrEqual(3);
    });
  }
  expect(taskColor('one')).toBe(taskColor('one'));
  expect(taskColor('one')).toMatch(/^var\(--dsw-alias-workspace-/);
});
