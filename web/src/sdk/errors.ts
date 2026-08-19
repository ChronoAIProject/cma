/**
 * What a caller is told when a request does not succeed.
 *
 * Two kinds, deliberately, because `CLAUDE.md` §3 turns on a caller being able
 * to tell them apart: **"this session can never do that"** and **"that just
 * failed"**. Only the second is worth retrying, only the first is worth
 * telling somebody at creation, and a client that collapsed them into one
 * `Error` would leave every consumer re-deriving the distinction from a string.
 */

/**
 * A request the service refused or could not answer.
 *
 * Carries the status so a caller can branch on it without parsing prose, and
 * the service's own `detail` where it sent one — a problem document says more
 * than the status line does, and discarding it in favour of "request failed"
 * throws away the only part anybody could act on.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  /**
   * Whether this means "sign in", as opposed to "something went wrong".
   *
   * A console reacts differently to the two: one is a prompt, the other is
   * something to report. Status 0 is neither — it is the network never having
   * reached the service, so nobody has said anything about the credential.
   */
  get isUnauthenticated(): boolean {
    return this.status === 401;
  }

  /** Whether retrying could plausibly succeed. */
  get isRetryable(): boolean {
    return this.status === 0 || this.status === 429 || this.status >= 500;
  }
}

/**
 * A session was asked for something it was never granted.
 *
 * Separate from `ApiError` because it is the permanent half of the pair.
 * `CLAUDE.md` §3 requires the refusal to name the ability in words a caller
 * can act on, and to arrive before anything is provisioned — never as a 404,
 * and never as silence. The name travels here so a surface can say *which*
 * ability is missing rather than "something went wrong".
 */
export class CapabilityRefused extends ApiError {
  constructor(
    status: number,
    message: string,
    /** The ability the session lacks, in the service's own word for it. */
    readonly capability?: string,
  ) {
    super(status, message);
    this.name = 'CapabilityRefused';
  }
}
