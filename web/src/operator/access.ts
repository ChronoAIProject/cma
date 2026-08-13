import type { Capability } from '../auth/types';
import type { AdminSession } from './types';

/**
 * Who is offered what, and which controls apply.
 *
 * Rules rather than layout, for the same reason the session abilities are kept
 * apart from the buttons that draw them: the menu item, the view switch and the
 * controls all answer the same question, and three copies of the answer
 * eventually disagree.
 *
 * **None of this protects anything.** Every read and every control on the
 * operator surface is refused by the control plane on its own capability. What
 * this decides is what is worth drawing — a control nobody can use should not be
 * offered, and a control that is drawn is not thereby permitted.
 */

/** Which of the panel's views this is. */
export type OperatorViewId = 'people' | 'activity' | 'namespace' | 'sessions';

/** One view of the operator panel, and what a caller must hold to see it. */
export interface OperatorView {
  readonly id: OperatorViewId;
  /** What the view switch calls it. */
  readonly label: string;
  /**
   * The capability that opens it.
   *
   * One each, matching the API: reading the directory, the record, the
   * inventory and every session are four separate grants and none implies
   * another, so a caller holding two sees two views.
   */
  readonly capability: Capability;
}

/**
 * The four views, in the order the panel offers them.
 *
 * People first because every other view names them, then what the service was
 * asked to do, then what is running, then the one view that acts.
 */
export const OPERATOR_VIEWS: readonly OperatorView[] = [
  { id: 'people', label: 'People', capability: 'read_user_directory' },
  { id: 'activity', label: 'Activity', capability: 'read_activity_log' },
  { id: 'namespace', label: 'Namespace', capability: 'read_runtime_inventory' },
  { id: 'sessions', label: 'Sessions', capability: 'read_all_sessions' },
];

/** The views this caller is offered. */
export function viewsFor(capabilities: readonly Capability[]): readonly OperatorView[] {
  return OPERATOR_VIEWS.filter((view) => capabilities.includes(view.capability));
}

/**
 * Whether the panel is worth offering this caller at all.
 *
 * Read capabilities only. Somebody holding nothing but the power to stop a
 * session cannot be shown one to stop — the panel has no view they may load —
 * and offering an empty dialog would be worse than not offering it: they would
 * open it looking for the thing they were told they could do.
 */
export function canOperate(capabilities: readonly Capability[]): boolean {
  return viewsFor(capabilities).length > 0;
}

/** Whether this caller may stop or end a session they do not own. */
export function canControlAnySession(capabilities: readonly Capability[]): boolean {
  return capabilities.includes('control_any_session');
}

/** What may be done to one session from the operator panel. */
export interface SessionControls {
  readonly canStop: boolean;
  readonly canTerminate: boolean;
  /** Why stopping is unavailable, when it is. */
  readonly stopReason?: string;
  /** Why ending is unavailable, when it is. */
  readonly terminateReason?: string;
}

/** Explanations shared by both controls, so they cannot drift apart. */
const NOT_ENTITLED = 'You may watch sessions but not act on them.';
const ALREADY_ENDED = 'This session has already ended.';

/**
 * Which controls apply to a session, and why they do not.
 *
 * A state this console has not been taught leaves both controls **enabled**.
 * That is deliberate and it is the opposite of what the owner's own panel does:
 * an operator reaching for this is usually reaching for something misbehaving,
 * and refusing locally would leave them with no way to act on a session in a
 * state a newer control plane has introduced. The control plane is the
 * authority and will refuse if it disagrees.
 */
export function controlsFor(
  session: AdminSession,
  capabilities: readonly Capability[],
): SessionControls {
  if (!canControlAnySession(capabilities)) {
    return {
      canStop: false,
      canTerminate: false,
      stopReason: NOT_ENTITLED,
      terminateReason: NOT_ENTITLED,
    };
  }
  switch (session.status) {
    case 'terminated':
      return {
        canStop: false,
        canTerminate: false,
        stopReason: ALREADY_ENDED,
        terminateReason: ALREADY_ENDED,
      };
    case 'idle':
      // Stopping releases a runtime, and this session already has none. Ending
      // it still means something: it stops the owner reviving it.
      return {
        canStop: false,
        canTerminate: true,
        stopReason: 'Its runtime is already released.',
      };
    default:
      return { canStop: true, canTerminate: true };
  }
}
