/**
 * Recorded conversations, for building the surface before its backend exists.
 *
 * A fixture states **how it came to exist** and does so in the file itself.
 * Replacing an authored fixture with a recorded one is a change nobody should
 * have to guess at, and a transcript whose provenance lives in a commit
 * message is a transcript whose provenance is lost the moment it is copied.
 */
export type Provenance =
  /**
   * Written by hand from `docs/API_AGUI.md`.
   *
   * Honest about what it is: it proves a component handles the shape the
   * document describes, and proves nothing about what a kernel actually emits.
   */
  | 'authored'
  /**
   * Captured from a running session.
   *
   * Proves the shape a kernel really produces. Worth more than an authored
   * one, and the reason the distinction is recorded rather than assumed.
   */
  | 'recorded';

export interface Transcript {
  /** How the surface refers to it. */
  readonly name: string;
  /** What it exercises, in one line. */
  readonly about: string;
  readonly provenance: Provenance;
  /**
   * Where a recorded one came from, or which part of the document an authored
   * one was written against.
   */
  readonly source: string;
  /**
   * The frames, in the order they happened, each with the delay before it.
   *
   * Delays are real, so streaming looks like streaming rather than appearing
   * whole — a surface that only ever sees a finished conversation is a surface
   * whose streaming behaviour nobody has looked at.
   */
  readonly frames: readonly Frame[];
}

export interface Frame {
  /** Milliseconds after the previous frame. */
  readonly afterMs: number;
  /** The frame exactly as the wire would carry it. */
  readonly event: Readonly<Record<string, unknown>>;
}
