import type { Session } from './types';
import type { Stage } from './progress';
import { stageLabel } from './progress';

/**
 * One line saying what a session last did — docs/Web_Style.md §12.
 *
 * "The last-activity line is what makes a list of times into a list of work."
 *
 * Composed from what the control plane actually reports rather than invented.
 * A session's representation carries its status, why it stopped, how many
 * times it has been rotated and the moment it resumed from; a starting session
 * additionally reports the step it has reached. That is the whole of what is
 * known about what a session has been doing, so it is the whole of what this
 * says.
 *
 * **It does not describe the agent's work.** No public interface reports what
 * a session's agent last did — `GET /api/v1/sessions/{id}` returns lifecycle
 * facts, and the transcript is a byte stream with no semantics above it. A
 * line claiming to summarise the work would be a claim nothing behind it can
 * support, so this reports the platform's own account and stops there. When an
 * interface begins projecting turn-level activity, this is the one place that
 * has to change.
 */
export function lastActivity(
  session: Session,
  stage?: Stage,
  waiting = false,
  signingIn = false,
): string {
  /*
   * A session whose kernel spends its owner's own vendor account stops inside
   * the conversation open to ask them to sign in, and waits there. It is
   * `provisioning` to the lifecycle and waiting on a person, so saying it
   * waits for a sandbox sends them to look at infrastructure while the thing
   * it needs is on their own screen. Same shape as the approval case above.
   */
  if (signingIn && session.lifecycle === 'provisioning') {
    return 'Waiting for you to sign in';
  }

  /*
   * §11 and §18.3: a session parked on an answerable approval is `idle` to
   * the lifecycle and the most alive a session gets to its owner. The line
   * says it is waiting for them, never that it stopped.
   */
  if ((waiting || session.stopReason === 'requires_action') && session.lifecycle === 'idle') {
    return 'Waiting for your answer';
  }

  switch (session.lifecycle) {
    case 'provisioning':
      return stage === undefined ? 'Waiting for a sandbox' : stageLabel(stage);

    case 'rotating':
      return 'Moving to an updated runtime';

    case 'ready':
      return 'Ready for its first turn';

    case 'running':
      return rotated(session) ?? 'Running';

    case 'idle':
      return 'Paused — its sandbox was released';

    case 'terminated':
      return session.stopReason === 'error' ? 'Ended after a failure' : 'Ended';

    case 'unknown':
      return session.reportedStatus ?? 'State unavailable';

    default: {
      const exhaustive: never = session.lifecycle;
      return exhaustive;
    }
  }
}

/**
 * What a session that has been carried forward last did, when that is the more
 * useful fact than "running".
 *
 * CLAUDE.md §5 requires the snapshot moment to be shown, because anything the
 * agent did after it did not travel: an owner who can see the boundary can
 * recognise a gap rather than puzzle over one.
 */
function rotated(session: Session): string | null {
  if (session.rotations === undefined || session.rotations === 0) {
    return null;
  }
  const times = session.rotations === 1 ? 'once' : `${String(session.rotations)} times`;
  return `Running — carried forward ${times}`;
}
