import { describe, expect, it } from 'vitest';
import { fullTimestamp, isSameDayInZone, sortSessions, tabLabel, timeOfDay } from './sessionTime';

/**
 * Session times are shown in one fixed zone, not the viewer's.
 *
 * A session is a shared object — someone may be told about it or read it in a
 * log — so a timestamp meaning a different moment depending on who is looking
 * is worse than one consistently in a stated zone. These tests pin that, since
 * a machine in another zone would otherwise quietly produce different labels.
 */

/** 2026-08-14 06:30 UTC, which is 14:30 in Singapore. */
const UTC_0630 = Date.parse('2026-08-14T06:30:00Z');
/** The same Singapore day, later on. */
const UTC_1000 = Date.parse('2026-08-14T10:00:00Z');
/** 2026-08-13 20:00 UTC, which is already the 14th in Singapore. */
const UTC_PREV_EVENING = Date.parse('2026-08-13T20:00:00Z');

describe('session timestamps', () => {
  it('shows the time in Singapore rather than the viewer’s zone', () => {
    // 06:30 UTC is 14:30 SGT. A machine running in UTC must still say 14:30.
    expect(tabLabel(UTC_0630, UTC_0630)).toBe('14:30');
  });

  it('shows only the time for a session started today', () => {
    // Within a working day the time is what distinguishes one from another.
    expect(tabLabel(UTC_0630, UTC_1000)).toBe('14:30');
  });

  it('adds the date once the session is no longer from today', () => {
    // "14:30" alone stops being enough to place it.
    const nextDay = Date.parse('2026-08-16T02:00:00Z');
    expect(tabLabel(UTC_0630, nextDay)).toMatch(/14 Aug 14:30/);
  });

  it('treats the Singapore day as the day, not the UTC one', () => {
    // 20:00 UTC on the 13th is already the 14th in Singapore; a naive
    // comparison would call these different days.
    expect(isSameDayInZone(UTC_PREV_EVENING, UTC_0630)).toBe(true);
  });

  it('states the zone on the full timestamp', () => {
    // Reference material has to say what it is relative to.
    expect(fullTimestamp(UTC_0630)).toContain('SGT');
  });

  it('gives the time of day a turn label reads', () => {
    // 04:00 UTC is 12:00 in Singapore — the same fixed zone, for the same
    // reason: a turn's time must not mean a different moment per viewer.
    expect(timeOfDay(Date.parse('2026-08-19T04:00:00Z'))).toBe('12:00');
  });
});

describe('session ordering', () => {
  const sessions = [
    { id: 'a', createdAt: 100 },
    { id: 'b', createdAt: 300 },
    { id: 'c', createdAt: 200 },
  ];

  it('puts the newest first by default', () => {
    // Almost always the one someone wants; making them scroll to it would be
    // the wrong default every time but the first.
    expect(sortSessions(sessions, 'newest').map((s) => s.id)).toEqual(['b', 'c', 'a']);
  });

  it('can show the oldest first', () => {
    expect(sortSessions(sessions, 'oldest').map((s) => s.id)).toEqual(['a', 'c', 'b']);
  });

  it('does not reorder the list it was given', () => {
    // The input belongs to React; sorting in place would mutate held state.
    const original = [...sessions];
    sortSessions(sessions, 'oldest');
    expect(sessions).toEqual(original);
  });

  it('handles an empty list', () => {
    expect(sortSessions([], 'newest')).toEqual([]);
  });
});
