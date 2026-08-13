/**
 * The contract between the terminal surface and whatever drives it.
 *
 * The surface owns rendering, hardening and geometry. A driver owns where
 * bytes come from and where keystrokes go. Neither knows the other's concrete
 * type, so replacing the mock driver with a real WebSocket relay (#14) touches
 * no component.
 */

/** What a driver may do to the surface. Implemented by the surface. */
export interface TerminalSink {
  /** Render bytes. Called with whatever the driver produces, unmodified. */
  write(data: string): void;
  /** Report a terminal condition the user must see rather than a silent freeze. */
  fail(message: string): void;
  /** Report that the driver finished normally. */
  finish(reason: string): void;
}

export interface TerminalDriver {
  /**
   * Begin producing output into the sink.
   * Implementations must tolerate being attached, detached and reattached.
   */
  attach(sink: TerminalSink): void;

  /** Deliver a user keystroke or paste. */
  send(data: string): void;

  /**
   * Inform the driver of new terminal geometry.
   * #18 requires this be sent before the first data frame.
   */
  resize(cols: number, rows: number): void;

  /** Stop producing and release resources. Must be safe to call twice. */
  detach(): void;
}
