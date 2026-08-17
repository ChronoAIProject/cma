import { modesOf, PLANES } from './kernels';
import type { KernelOffer, Session, SessionPlane } from './types';

/**
 * Whether a session can be moved to the other plane, and what that would cost
 * — `Web_Style.md` §18.6.
 *
 * Kept apart from the control that draws it because it answers a question the
 * control should not be deciding: a switch is offered only where the session's
 * kernel actually advertises the other plane, and where the session is in a
 * state that can be moved at all. §18.6 is explicit that the affordance is
 * **absent, not disabled**, where the kernel cannot serve the other plane —
 * for §18.4's reason, that a control which can never work is not one that is
 * currently unavailable.
 */

export interface PlaneSwitch {
  /** The plane it would move to. */
  readonly to: SessionPlane;
  /** What that plane is called, for the key that offers it. */
  readonly label: string;
  /**
   * Whether it can be asked for right now.
   *
   * False where the session is in a state that cannot be moved — mid-rotation,
   * stopped, or ended. Drawn disabled with the state's own reason rather than
   * removed, which is the opposite of the kernel case: this one *will* work
   * again, and a control that vanishes and returns is one somebody hunts for.
   */
  readonly available: boolean;
}

/** The plane this session is on, falling back to what every session once was. */
export function planeOf(session: Session): SessionPlane {
  return session.plane ?? 'terminal';
}

/**
 * The switch this session offers, or `undefined` where it offers none.
 *
 * `undefined` means the kernel does not advertise the other plane, or nothing
 * is known about the kernel at all. Both draw nothing: offering a move that
 * will be refused is worse than not offering it, and guessing from an unread
 * runtime list would do exactly that.
 */
export function switchFor(
  session: Session,
  kernels: readonly KernelOffer[],
): PlaneSwitch | undefined {
  const to: SessionPlane = planeOf(session) === 'terminal' ? 'conversation' : 'terminal';
  const modes = modesOf(kernels, session.kernel);
  if (modes === undefined || !modes.includes(to)) {
    return undefined;
  }
  return {
    to,
    label: PLANES.find((plane) => plane.id === to)?.label ?? to,
    // A move is a rotation of a live runtime. A session with none has nothing
    // to carry: it is started on whichever plane its profile names when it is
    // revived, which the core says by refusing this outright.
    available: session.lifecycle === 'ready' || session.lifecycle === 'running',
  };
}

/**
 * Why a switch cannot be asked for right now, in the owner's terms.
 *
 * Only ever called where the switch exists and is unavailable, so it explains
 * a state rather than a missing ability — the two are different answers and
 * §18.4 keeps them apart.
 */
export function unavailableBecause(session: Session): string {
  switch (session.lifecycle) {
    case 'rotating':
      return 'This session is already being moved.';
    case 'provisioning':
      return 'This session is still starting.';
    case 'idle':
      return 'Revive this session before moving it.';
    case 'terminated':
      return 'This session has ended.';
    default:
      return 'This session cannot be moved right now.';
  }
}
