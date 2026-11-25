import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DesktopPanel } from './DesktopPanel';

const DateTimeFormat = Intl.DateTimeFormat;
let timeZone: string;
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-24T04:09:59.500Z'));
  timeZone = 'Asia/Tokyo';
  vi.spyOn(Intl, 'DateTimeFormat').mockImplementation(function (_, options) {
    return new DateTimeFormat('en-GB', { ...options, timeZone });
  });
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function draw(hidden = false) {
  return (
    <DesktopPanel
      runtimeReady={false}
      agentId="agt_clock"
      running={false}
      onClose={vi.fn()}
      embedded
      hidden={hidden}
    />
  );
}

it('shows device-local time and timezone in the desktop header without a live announcement', () => {
  const view = render(draw());
  const clock = screen.getByText('Tue 13:09');
  expect(clock.tagName).toBe('TIME');
  expect(clock).toHaveAttribute('datetime', '2026-11-24T04:09:59.500Z');
  expect(clock.closest('header')).toHaveTextContent('Local time · Asia/Tokyo');
  expect(clock.closest('p')).toHaveAttribute(
    'title',
    'Your device’s local time. The desktop clock follows the sandbox timezone.',
  );
  expect(clock.closest('[role="status"], [aria-live]')).toBeNull();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it.each([
  ['Asia/Tokyo', '2026-11-24T14:59:59.500Z', 'Tue 23:59', 'Wed 00:00'],
  ['America/New_York', '2026-03-08T06:59:59.500Z', 'Sun 01:59', 'Sun 03:00'],
  ['America/New_York', '2026-11-01T05:59:59.500Z', 'Sun 01:59', 'Sun 01:00'],
])(
  'refreshes at the minute boundary through %s transitions from %s',
  (zone, instant, before, after) => {
    timeZone = zone;
    vi.setSystemTime(new Date(instant));
    const view = render(draw());
    expect(screen.getByText(before)).toBeVisible();
    act(() => vi.advanceTimersByTime(499));
    expect(screen.getByText(before)).toBeVisible();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.getByText(after)).toBeVisible();
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
  },
);

it('refreshes after backgrounding and detects a changed device timezone', () => {
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const view = render(draw());
  visibility.mockReturnValue('hidden');
  fireEvent(document, new Event('visibilitychange'));
  expect(vi.getTimerCount()).toBe(0);
  vi.setSystemTime(new Date('2026-11-25T01:10:00Z'));
  timeZone = 'America/Los_Angeles';
  visibility.mockReturnValue('visible');
  fireEvent(document, new Event('visibilitychange'));
  expect(screen.getByText('Tue 17:10').closest('header')).toHaveTextContent(
    'Local time · America/Los_Angeles',
  );
  view.unmount();
  fireEvent(document, new Event('visibilitychange'));
  expect(vi.getTimerCount()).toBe(0);
});

it('stops the clock while the desktop is hidden and reads fresh time on return', () => {
  const view = render(draw());
  view.rerender(draw(true));
  expect(vi.getTimerCount()).toBe(0);
  vi.setSystemTime(new Date('2026-11-24T05:10:00Z'));
  view.rerender(draw());
  expect(screen.getByText('Tue 14:10')).toBeVisible();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
