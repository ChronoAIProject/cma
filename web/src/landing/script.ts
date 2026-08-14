/**
 * The landing introduction: what is printed, in what colours, at what pace.
 *
 * Content lives here as data rather than as markup so the timing model and the
 * copy can be tested without rendering anything.
 *
 * The central decision is that only the two greeting lines are typed. The boot
 * block prints whole lines at a stagger, because real machine output (dmesg,
 * systemd) snaps in line at a time — character-by-character boot text is the
 * film version of a terminal, and it is where both the cheesiness and most of
 * the seconds would come from. Typing is reserved for the lines carrying a
 * voice.
 */

import { interfaceLine } from '../interfaces';

/** Which token a run of text is painted with. */
export type Tone = 'ink' | 'dim' | 'accent' | 'emphasis' | 'ok';

export interface Segment {
  readonly text: string;
  readonly tone: Tone;
}

export interface Line {
  readonly segments: readonly Segment[];
  /** Typed character by character, rather than printed whole. */
  readonly typed?: boolean;
}

/**
 * Every boot line is a true statement about the product, terminal-formatted.
 * Nothing here is invented atmosphere: the kernels and interfaces are the real
 * ones from docs/API_Overview.md, and `identity` states the visitor's actual
 * auth status, which plants sign-in without nagging about it.
 */
const BOOT: readonly Line[] = [
  {
    segments: [
      { text: 'chrono-bot', tone: 'accent' },
      { text: ' // ', tone: 'dim' },
      { text: 'chronoai · control plane', tone: 'ink' },
    ],
  },
  { segments: [{ text: '─'.repeat(42), tone: 'dim' }] },
  {
    segments: [
      { text: 'kernel     ', tone: 'dim' },
      { text: 'codex · claude-code', tone: 'ink' },
    ],
  },
  {
    segments: [
      { text: 'interface  ', tone: 'dim' },
      // Derived, not transcribed: adding a Layer 1 interface should surface
      // here without anyone remembering to edit this line.
      { text: interfaceLine(), tone: 'ink' },
    ],
  },
  {
    segments: [
      { text: 'identity   ', tone: 'dim' },
      { text: 'nyxid · ', tone: 'ink' },
      { text: 'guest', tone: 'emphasis' },
    ],
  },
  {
    segments: [
      { text: 'install    ', tone: 'dim' },
      { text: 'none', tone: 'ok' },
      { text: ' — the machine is already running', tone: 'ink' },
    ],
  },
];

/** The greeting. These are the only lines that type. */
const GREETING: readonly Line[] = [
  { segments: [], typed: true },
  {
    typed: true,
    segments: [
      { text: "Hi, I'm ", tone: 'ink' },
      { text: 'ChronoBot', tone: 'emphasis' },
      { text: " — I'm a managed agent provided by ChronoAI.", tone: 'ink' },
    ],
  },
  {
    typed: true,
    segments: [{ text: 'How can I help you today?', tone: 'ink' }],
  },
];

export const INTRO_LINES: readonly Line[] = [...BOOT, ...GREETING];

/** Count of boot lines, which print rather than type. */
export const BOOT_LINE_COUNT = BOOT.length;

/**
 * Timing. Every value is deliberate; see the justification on each.
 */
export const TIMING = {
  /** A solid caret on an empty screen, so the first line reads as an event
   *  rather than as content that was always there. Under the ~150ms at which
   *  lag starts to be perceived, so it costs nothing. */
  preRoll: 80,

  /** Above the ~50ms at which successive visual events fuse into one, so each
   *  boot line lands separately. Five gaps total 375ms, inside the 400ms
   *  Doherty threshold, so the block still reads as instantaneous. */
  bootStagger: 75,

  /** The handover from machine voice to persona. Above the ~200ms at which a
   *  gap becomes consciously perceptible, so it reads as intentional. */
  turnBeat: 220,

  /** 50 characters per second, about 550wpm. Adult silent reading averages
   *  ~238wpm, so the print runs roughly twice as fast as the fastest
   *  comfortable reader and is never the rate limiter. Below ~35ms/char the
   *  animation itself becomes the bottleneck, which is what kills a landing
   *  page. */
  charMs: 20,

  /** Prosodic pauses from read-aloud speech, compressed to match the print
   *  speed. They also rest the eye exactly at the syntactic boundaries where
   *  regressive saccades cluster. */
  holdComma: 90,
  holdDash: 90,
  holdSentence: 180,
  holdEol: 140,

  /** Beat before the prompt appears. */
  promptReveal: 120,

  /** Matches the platform default and the DEC/xterm convention. 0.94Hz, far
   *  below the WCAG 2.3.1 three-flash threshold. */
  caretBlink: 530,

  /** The skip control appears only once the boot block has finished — the
   *  first moment at which "how long is this?" can occur to anyone. Showing it
   *  at 0ms would advertise the animation as an obstacle before the visitor
   *  had decided it was one. */
  skipRevealAt: 500,

  /** The Enter hint waits for real intent: three characters, then a pause.
   *  Revealing it mid-keystroke puts motion in peripheral vision at the exact
   *  moment someone is composing, which is the definition of nagging. */
  hintMinChars: 3,
  hintIdleMs: 400,

  /** Pace of the opening line typing itself into the prompt. Slower than the
   *  boot text, because this is meant to read as someone typing rather than as
   *  a machine printing. */
  typeInterval: 55,
} as const;

/**
 * What the prompt suggests, typed in once the introduction finishes.
 *
 * A suggestion rather than a script: it stays editable, and a visitor who has
 * something else in mind can replace it. An empty field with a blinking caret
 * asks a question the page has not yet earned.
 */
export const OPENING_LINE = "Let's get started";

/** Extra pause owed after a given character. */
export function holdAfter(char: string): number {
  if (char === '.' || char === '!' || char === '?') {
    return TIMING.holdSentence;
  }
  if (char === ',') {
    return TIMING.holdComma;
  }
  if (char === '—') {
    return TIMING.holdDash;
  }
  return 0;
}

/** Plain text of a line, for measurement and for the accessible transcript. */
export function lineText(line: Line): string {
  return line.segments.map((s) => s.text).join('');
}

/** Total characters that will be typed, used to schedule the animation. */
export function typedCharCount(): number {
  return INTRO_LINES.filter((l) => l.typed === true).reduce(
    (sum, l) => sum + lineText(l).length,
    0,
  );
}

/**
 * Prose equivalent for assistive technology.
 *
 * Deliberately not the visual script: box-drawing rules and `·` separators read
 * as noise through a screen reader. This is present in the DOM at first paint,
 * before any animation starts, and is not a live region — so the virtual
 * cursor can read, re-read and navigate it from load.
 */
export const ACCESSIBLE_TRANSCRIPT: readonly string[] = [
  'chrono-bot, a managed agent runtime from ChronoAI.',
  'Kernels available: Codex and Claude Code.',
  'Interfaces: AG-UI, REST, PTY, MCP and A2A.',
  'Identity is provided by NyxID. You are not signed in.',
  'No local installation is required — the agent runs on ChronoAI infrastructure.',
  "ChronoBot says: Hi, I'm ChronoBot — I'm a managed agent provided by ChronoAI. How can I help you today?",
];
