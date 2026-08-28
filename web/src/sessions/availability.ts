import type { Session } from './types';

/** What `GET /api/v1/sessions/allowance` permits this caller. */
export interface Allowance {
  /** Absent when the deployment sets no limit. */
  readonly limit?: number;
  readonly live: number;
  readonly exempt: boolean;
}

/** Whether another session may be started, and why not. */
export interface CreationAbility {
  readonly canCreate: boolean;
  /** Present exactly when `canCreate` is false, in the owner's terms. */
  readonly reason?: string;
}

/**
 * Decide whether to offer another session from one advisory answer.
 *
 * `undefined` means the client could not ask, and it enables: a failed
 * courtesy read must not disable the only way to start work. The create
 * request remains authoritative in every case.
 */
export function creationAbilityFor(allowance: Allowance | undefined): CreationAbility {
  if (allowance === undefined || allowance.exempt || allowance.limit === undefined) {
    return { canCreate: true };
  }
  const { limit, live } = allowance;
  if (live < limit) {
    return { canCreate: true };
  }
  return {
    canCreate: false,
    reason:
      live === limit
        ? `You have ${String(live)} of ${String(limit)} sessions running. Pause or end one to start another.`
        : `You have ${String(live)} sessions running and the limit is ${String(limit)}. Pause or end sessions until fewer than ${String(limit)} are running.`,
  };
}

/**
 * What can be done to a session in each of its states.
 *
 * Separated from the buttons that render it because it is a rule, not a
 * layout. The same rule decides whether a control is offered, whether it is
 * enabled, and whether the terminal accepts typing — and three copies of it
 * would eventually disagree, which shows up as a button that does nothing.
 *
 * Every state answers every question. A state that answered only some would
 * leave the interface guessing about the rest, and guessing here means either
 * offering an action that fails or hiding one that would have worked.
 */

export interface SessionAbilities {
  /** Whether the session can be typed into right now. */
  readonly interactive: boolean;
  /** Whether reattaching to a running sandbox makes sense. */
  readonly canReconnect: boolean;
  /** Whether the sandbox can be released, keeping the session. */
  readonly canStop: boolean;
  /** Whether a released session can be given a sandbox again. */
  readonly canRevive: boolean;
  /** Whether the session and its history can be removed. */
  readonly canDelete: boolean;
  /** Whether it belongs in the owner's session list. */
  readonly listed: boolean;
  /**
   * Why the session cannot be typed into, in the owner's terms.
   *
   * Present exactly when `interactive` is false. A disabled terminal with no
   * explanation is the state people report as "it stopped working".
   */
  readonly reason?: string;
}

export function abilitiesFor(session: Session, signingIn = false): SessionAbilities {
  switch (session.lifecycle) {
    case 'provisioning':
      // The sandbox is not there yet, so there is nothing to type into and
      // nothing to reconnect to. Stopping is still allowed: waiting several
      // minutes for something you no longer want is its own trap.
      return {
        interactive: false,
        canReconnect: false,
        canStop: true,
        canRevive: false,
        canDelete: true,
        listed: true,
        /*
         * A session waiting on its owner's sign-in is `provisioning` too, and
         * telling them it is starting sends them to wait for something that
         * is waiting for them. The abilities are identical either way — there
         * is still nothing to type into — so only the sentence changes.
         */
        reason: signingIn
          ? 'This session is waiting for you to sign in.'
          : 'This session is still starting.',
      };
    case 'ready':
    case 'running':
      return {
        interactive: true,
        canReconnect: true,
        canStop: true,
        canRevive: false,
        canDelete: true,
        listed: true,
      };
    case 'rotating':
      // Input during a rotation would go to a sandbox being torn down: it
      // would be accepted and then lost, which is worse than being briefly
      // refused. Stopping waits too, so that a stop cannot race the handover.
      return {
        interactive: false,
        canReconnect: false,
        canStop: false,
        canRevive: false,
        canDelete: false,
        listed: true,
        // Both handovers land here — a newer runtime, and the other plane —
        // and the record does not say which. Saying "an updated runtime"
        // would be a guess that is wrong half the time now, and the part
        // an owner acts on is the same either way: it is moving, and it
        // will be back.
        reason: 'This session is being moved to another runtime. It will come back.',
      };
    case 'idle': {
      // `requires_action` is parked on the client, not stopped. It stays
      // readable but never offers Resume for a runtime that is already waiting.
      const waitingForClient = session.stopReason === 'requires_action';
      return {
        interactive: false,
        canReconnect: false,
        canStop: false,
        canRevive: !waitingForClient,
        canDelete: true,
        listed: true,
        reason: waitingForClient
          ? 'This session is waiting for you.'
          : 'This session is paused. Resume it to carry on.',
      };
    }
    case 'terminated':
      return {
        interactive: false,
        canReconnect: false,
        canStop: false,
        canRevive: false,
        canDelete: true,
        // Ended sessions leave the list, because a list of everything anyone
        // has ever run is not a list of their sessions. The exception is one
        // that ended by failing: hiding those would answer "where did my
        // session go" by silently removing the evidence.
        listed: session.stopReason === 'error',
        reason:
          session.stopReason === 'error'
            ? 'This session ended because it failed.'
            : 'This session has ended.',
      };
    case 'unknown':
      return {
        interactive: false,
        canReconnect: false,
        canStop: false,
        canRevive: false,
        canDelete: false,
        listed: true,
        reason: 'This session is in an unrecognised state.',
      };
    default: {
      // Exhaustiveness: a new state must be given abilities deliberately
      // rather than inheriting whatever the last case happened to be.
      const exhaustive: never = session.lifecycle;
      return exhaustive;
    }
  }
}

/** Sessions to show in the rail, in the order given. */
export function listable(sessions: readonly Session[]): readonly Session[] {
  return sessions.filter((s) => abilitiesFor(s).listed);
}
