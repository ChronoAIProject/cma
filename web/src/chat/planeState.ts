import type { Session } from '../sessions/types';
import { abilitiesFor } from '../sessions/availability';

/**
 * What a session's state allows on the conversation plane — `Web_Style.md`
 * §11 and §18.6.
 *
 * §11's table covers **both planes** deliberately, so this reads
 * `abilitiesFor` rather than restating it: a second copy of the rule would be
 * a second place for the planes to disagree about what a state means, which is
 * exactly what §11 grew a column to avoid.
 *
 * Three states are load-bearing and are routinely got wrong.
 */

export interface PlaneState {
  /** Whether the composer may accept and send. */
  readonly interactive: boolean;
  /**
   * Why it may not, in the owner's terms.
   *
   * Present exactly when `interactive` is false. §11: a session that cannot be
   * typed into always says why, on the surface showing it.
   */
  readonly reason?: string;
  /**
   * Whether this is a failure.
   *
   * `idle` and `recovering` are not, and neither is `unknown` with
   * `runtime_observation_unavailable`. Drawing any of them as an error trains
   * owners to ignore real ones.
   */
  readonly failed: boolean;
  /**
   * Whether the session is parked on an answerable approval — §11 and §18.3.
   *
   * The wording of a refusal belongs to the reason it exists, not to the
   * state's name: a session waiting on an answer says it is waiting for its
   * owner, in the rail, the header and at the composer, rather than claiming
   * to be stopped while an approval it can answer sits above.
   */
  readonly waiting: boolean;
}

export function planeStateFor(session: Session, hasPendingApproval = false): PlaneState {
  const can = abilitiesFor(session);

  if (isWaitingForAnswer(session, hasPendingApproval)) {
    /*
     * The live defect this section exists to fix: `idle` used to answer for
     * this case, so the composer said "This session is stopped. Revive it to
     * carry on." while an answerable approval sat directly above it.
     */
    return {
      interactive: false,
      reason: 'The agent stopped here on purpose — it is waiting for your answer above.',
      failed: false,
      waiting: true,
    };
  }

  if (session.lifecycle === 'rotating') {
    /*
     * `API_Overview.md` §2 does not describe this state, it instructs:
     * "Clients must refuse input while it lasts — a keystroke sent now reaches
     * a sandbox being torn down, so it would be accepted and then lost."
     * CLAUDE.md §5 puts it more bluntly still. Refusing is the requirement;
     * saying why is the design.
     */
    return {
      interactive: false,
      reason:
        'This session is being moved to another runtime. It cannot take input until it lands.',
      failed: false,
      waiting: false,
    };
  }

  return {
    interactive: can.interactive,
    reason: can.reason,
    /*
     * Only a session that ended *because it failed* is a failure. `idle` is a
     * session waiting for its owner; `recovering` with `runtime_absent` is
     * normal during an idle-to-revive cycle; and a session the control plane
     * cannot currently see is not a session that is dead.
     */
    failed: session.lifecycle === 'terminated' && session.stopReason === 'error',
    waiting: false,
  };
}

/**
 * Whether a session is parked on an approval rather than merely idle.
 *
 * §11's table: `idle` is history and read-only on the conversation plane —
 * *unless it is waiting on an approval*, which is the most alive a session
 * gets and the one case where an idle session has a control to offer.
 */
export function isWaitingForAnswer(session: Session, hasPendingApproval: boolean): boolean {
  return session.lifecycle === 'idle' && hasPendingApproval;
}
