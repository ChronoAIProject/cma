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
    /** The service's stable problem code, where it sent one. */
    readonly code?: string,
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

/**
 * This owner already holds as many live sessions as the deployment permits.
 *
 * The numbers remain fields rather than being recovered from `message`: the
 * service deliberately gives over-limit owners a different sentence, and a
 * client should never have to parse either one to act on the refusal.
 */
export class SessionLimitReached extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly limit?: number,
    readonly live?: number,
  ) {
    super(status, message);
    this.name = 'SessionLimitReached';
  }
}

/**
 * This deployment has less declared room than one creation asks for.
 *
 * Distinct from an owner's session limit: these figures describe shared
 * deployment headroom, so stopping somebody else's work can change the answer.
 * They remain fields so a surface can state the request and the available room
 * without parsing the control plane's sentence.
 */
export class DeploymentAtCapacity extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly requestedCpuMillis?: number,
    readonly requestedMemoryMib?: number,
    readonly freeCpuMillis?: number,
    readonly freeMemoryMib?: number,
  ) {
    super(status, message, 'deployment_at_capacity');
    this.name = 'DeploymentAtCapacity';
  }
}
