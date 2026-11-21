import { expect, it } from 'vitest';
import { relativeMessageTime } from './relativeTime';
const at = '2026-11-05T00:00:00Z';
it.each([
  [-1, 'just now'],
  [0, 'just now'],
  [59, 'just now'],
  [60, '1 minute ago'],
  [300, '5 minutes ago'],
  [3599, '59 minutes ago'],
  [3600, '1 hour ago'],
  [10800, '3 hours ago'],
  [86399, '23 hours ago'],
  [86400, '1 day ago'],
  [172800, '2 days ago'],
])('formats %s elapsed seconds consistently', (seconds, expected) => {
  expect(relativeMessageTime(at, Date.parse(at) + seconds * 1000)).toBe(expected);
});
it('does not invent missing or invalid message times', () => {
  expect(relativeMessageTime(undefined, 0)).toBeUndefined();
  expect(relativeMessageTime('not a date', 0)).toBeUndefined();
});
