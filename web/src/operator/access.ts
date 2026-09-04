import type { Capability } from '../auth/types';
import type { AdminSession, ChatHost } from './types';

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
export type OperatorViewId =
  'people' | 'activity' | 'namespace' | 'sessions' | 'chat_hosts' | 'settings' | 'chat_settings';

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
 * The six views, in the order the panel offers them.
 *
 * People first because every other record view names them, then what the
 * service was asked to do, then what is running, then the view that acts on
 * sessions. Deployment policy follows those operational records.
 */
export const OPERATOR_VIEWS: readonly OperatorView[] = [
  { id: 'people', label: 'People', capability: 'read_user_directory' },
  { id: 'activity', label: 'Activity', capability: 'read_activity_log' },
  { id: 'namespace', label: 'Namespace', capability: 'read_runtime_inventory' },
  { id: 'sessions', label: 'Sessions', capability: 'read_all_sessions' },
  { id: 'chat_hosts', label: 'Chat hosts', capability: 'read_all_sessions' },
  { id: 'settings', label: 'Settings', capability: 'configure_platform' },
  { id: 'chat_settings', label: 'Chat', capability: 'configure_platform' },
];

/** The views this caller is offered. */
export function viewsFor(capabilities: readonly Capability[]): readonly OperatorView[] {
  return OPERATOR_VIEWS.filter((view) => capabilities.includes(view.capability));
}

/**
 * Whether the panel is worth offering this caller at all.
 *
 * View capabilities only. Somebody holding nothing but the power to stop a
 * session cannot be shown one to stop — the panel has no view they may load —
 * and offering an empty page would be worse than not offering it: they would
 * open it looking for the thing they were told they could do. Configuring the
 * platform is itself a view, so it does not require an unrelated read grant.
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
  readonly canDeleteChat: boolean;
  /** Why stopping is unavailable, when it is. */
  readonly stopReason?: string;
  /** Why ending is unavailable, when it is. */
  readonly terminateReason?: string;
  readonly deleteChatReason?: string;
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
      canDeleteChat: false,
      stopReason: NOT_ENTITLED,
      terminateReason: NOT_ENTITLED,
      deleteChatReason: session.chat ? NOT_ENTITLED : undefined,
    };
  }
  switch (session.status) {
    case 'terminated':
      return {
        canStop: false,
        canTerminate: false,
        canDeleteChat: session.chat,
        stopReason: ALREADY_ENDED,
        terminateReason: ALREADY_ENDED,
      };
    case 'idle':
      // Stopping releases a runtime, and this session already has none. Ending
      // it still means something: it stops the owner reviving it.
      return {
        canStop: false,
        canTerminate: true,
        canDeleteChat: session.chat,
        stopReason: 'Its runtime is already released.',
      };
    default:
      return { canStop: true, canTerminate: true, canDeleteChat: session.chat };
  }
}

/** Whether an operator may permanently reset one owner's complete Chat environment. */
export function environmentResetFor(capabilities: readonly Capability[]): {
  readonly canReset: boolean;
  readonly reason?: string;
} {
  return canControlAnySession(capabilities)
    ? { canReset: true }
    : { canReset: false, reason: NOT_ENTITLED };
}

/** Whether a host may be retired from the inventory, and why not when it may not. */
export function retirementFor(
  host: ChatHost,
  capabilities: readonly Capability[],
): { readonly canRetire: boolean; readonly reason?: string } {
  if (!canControlAnySession(capabilities)) {
    return { canRetire: false, reason: NOT_ENTITLED };
  }
  if (host.status === 'retired') {
    return { canRetire: false, reason: 'This chat host is already retired.' };
  }
  if (host.status !== 'active' && host.status !== 'paused') {
    return {
      canRetire: false,
      reason: 'A chat host can be retired only while active or paused.',
    };
  }
  return { canRetire: true };
}
