import { INTRO_LINES, TIMING, holdAfter, lineText } from './script';

/**
 * Precomputed reveal timetable for the introduction.
 *
 * Computing the whole schedule up front, rather than chaining timeouts, is what
 * makes the animation frame-rate independent: the render loop asks "what should
 * be visible at t?" instead of accumulating drift one setTimeout at a time. It
 * also makes the timing directly testable without rendering or faking clocks.
 */

export interface IntroSchedule {
  /** Offset in ms at which each line becomes visible at all. */
  readonly lineAt: readonly number[];
  /**
   * For typed lines, the offset at which each character index is revealed.
   * Indexed by line, then by character. Empty for printed lines.
   */
  readonly charAt: readonly (readonly number[])[];
  /** Offset at which the prompt goes live and the animation is complete. */
  readonly completeAt: number;
}

/** Deterministic jitter, so the print reads as transmission texture rather
 *  than as a marquee. Seeded rather than random so tests are reproducible and
 *  so two visitors see the same page. Stays under the ~25ms at which rhythm
 *  irregularity becomes consciously noticeable. */
function jitter(index: number): number {
  return ((index * 2654435761) % 11) - 5;
}

export function buildSchedule(): IntroSchedule {
  const lineAt: number[] = [];
  const charAt: number[][] = [];

  let clock = TIMING.preRoll;
  let startedTyping = false;

  INTRO_LINES.forEach((line, lineIndex) => {
    const typed = line.typed === true;

    if (typed && !startedTyping) {
      // One beat to mark the handover from machine output to a voice.
      clock += TIMING.turnBeat;
      startedTyping = true;
    }

    lineAt[lineIndex] = clock;

    if (!typed) {
      charAt[lineIndex] = [];
      clock += TIMING.bootStagger;
      return;
    }

    const text = lineText(line);
    const offsets: number[] = [];
    for (let i = 0; i < text.length; i += 1) {
      clock += TIMING.charMs + jitter(i);
      offsets.push(clock);
      clock += holdAfter(text[i] ?? '');
    }
    charAt[lineIndex] = offsets;

    // End-of-line rest, never summed with a sentence hold that just applied.
    const last = text.at(-1) ?? '';
    clock += Math.max(0, TIMING.holdEol - holdAfter(last));
  });

  return {
    lineAt,
    charAt,
    completeAt: clock + TIMING.promptReveal,
  };
}

/** How much of the script should be visible at a given offset. */
export interface IntroFrame {
  /** Number of lines that have begun appearing. */
  readonly visibleLines: number;
  /** Characters revealed on the last visible typed line. */
  readonly charsOnLastLine: number;
  readonly complete: boolean;
}

export function frameAt(schedule: IntroSchedule, elapsed: number): IntroFrame {
  if (elapsed >= schedule.completeAt) {
    return {
      visibleLines: INTRO_LINES.length,
      charsOnLastLine: Number.MAX_SAFE_INTEGER,
      complete: true,
    };
  }

  let visibleLines = 0;
  for (let i = 0; i < schedule.lineAt.length; i += 1) {
    if (elapsed >= (schedule.lineAt[i] ?? Infinity)) {
      visibleLines = i + 1;
    }
  }

  const lastIndex = visibleLines - 1;
  const offsets = lastIndex >= 0 ? (schedule.charAt[lastIndex] ?? []) : [];
  let charsOnLastLine = 0;
  for (const offset of offsets) {
    if (elapsed >= offset) {
      charsOnLastLine += 1;
    } else {
      break;
    }
  }

  return { visibleLines, charsOnLastLine, complete: false };
}
