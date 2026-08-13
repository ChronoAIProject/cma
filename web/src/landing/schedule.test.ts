import { describe, expect, it } from 'vitest';
import { buildSchedule, frameAt } from './schedule';
import { BOOT_LINE_COUNT, INTRO_LINES, TIMING, typedCharCount } from './script';

describe('intro schedule', () => {
  const schedule = buildSchedule();

  it('finishes well inside the four-second budget', () => {
    // A landing page that makes a visitor wait has already lost them. This is
    // the number that must not drift upward as copy is edited.
    expect(schedule.completeAt).toBeLessThan(4000);
  });

  it('prints the whole boot block inside the Doherty threshold', () => {
    // The boot lines should read as one instantaneous cascade, not a sequence
    // the visitor waits through.
    const lastBoot = schedule.lineAt[BOOT_LINE_COUNT - 1] ?? 0;
    expect(lastBoot - TIMING.preRoll).toBeLessThanOrEqual(400);
  });

  it('reveals lines in order', () => {
    for (let i = 1; i < schedule.lineAt.length; i += 1) {
      expect(schedule.lineAt[i]).toBeGreaterThanOrEqual(schedule.lineAt[i - 1] ?? 0);
    }
  });

  it('types only the greeting, not the boot block', () => {
    for (let i = 0; i < BOOT_LINE_COUNT; i += 1) {
      expect(schedule.charAt[i]).toEqual([]);
    }
    const typedLines = INTRO_LINES.filter((l) => l.typed === true);
    expect(typedLines.length).toBeGreaterThan(0);
  });

  it('schedules exactly one reveal per typed character', () => {
    const scheduled = schedule.charAt.reduce((sum, offsets) => sum + offsets.length, 0);
    expect(scheduled).toBe(typedCharCount());
  });

  it('shows nothing before the pre-roll', () => {
    expect(frameAt(schedule, 0).visibleLines).toBe(0);
    expect(frameAt(schedule, TIMING.preRoll - 1).visibleLines).toBe(0);
  });

  it('reports completion once the schedule is exhausted', () => {
    expect(frameAt(schedule, schedule.completeAt).complete).toBe(true);
    expect(frameAt(schedule, schedule.completeAt - 1).complete).toBe(false);
  });

  it('reveals characters monotonically', () => {
    let previous = -1;
    for (let t = 0; t <= schedule.completeAt; t += 25) {
      const frame = frameAt(schedule, t);
      if (frame.complete) {
        break;
      }
      const progress = frame.visibleLines * 10_000 + frame.charsOnLastLine;
      expect(progress).toBeGreaterThanOrEqual(previous);
      previous = progress;
    }
  });

  it('is deterministic, so two visitors see the same page', () => {
    expect(buildSchedule()).toEqual(buildSchedule());
  });
});
