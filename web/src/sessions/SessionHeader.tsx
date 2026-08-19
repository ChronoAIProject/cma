import type { ReactNode } from 'react';
import type { Session } from './types';
import { labelFor, stateDotFor, toneFor } from './types';
import { Pill } from '../ui/Pill';
import { StateDot } from '../ui/StateDot';
import { tabLabel } from './sessionTime';
import { planeOf, switchFor, unavailableBecause } from './planeSwitch';
import { PLANES } from './kernels';
import type { KernelOffer, SessionPlane } from './types';
import './SessionHeader.css';

/**
 * The centre column's own header — `docs/Web_Style.md` §11.
 *
 * Not chrome. It belongs to the column beneath it and scrolls nothing: the
 * crumb says which session is showing, the tabs say which plane, and the
 * trailing cluster holds what acts on it.
 *
 * The controls are here rather than in the details column because §12 puts
 * them "at the head of the surface they act on, visible without opening
 * anything". In a panel they were reachable only by opening reference
 * material, so ending a session meant first opening something nobody wanted
 * to read.
 *
 * **The tab strip is drawn only when there is more than one plane to choose.**
 * A strip with a single tab is a control that cannot do anything, and §7 says
 * what can never work is absent rather than disabled.
 */

interface SessionHeaderProps {
  /** The session the surface beneath is showing, or none. */
  readonly session: Session | null;
  /**
   * Whether it is parked on an answerable approval — §18.3: the pill reads
   * "Waiting for you" rather than the state's own name, because the wording
   * of a refusal belongs to the reason it exists.
   */
  readonly waiting?: boolean;
  /** The lifecycle controls that act on it. */
  readonly actions?: ReactNode;
  /** What each kernel offers, for deciding whether there are two planes. */
  readonly kernels?: readonly KernelOffer[];
  /** Absent where this client cannot move a session between planes. */
  readonly onChangePlane?: (to: SessionPlane) => void;
}

export function SessionHeader({
  session,
  waiting = false,
  actions,
  kernels = [],
  onChangePlane,
}: SessionHeaderProps) {
  if (session === null) {
    return (
      <div className="sessionheader">
        <span className="sessionheader__crumb">No session open</span>
      </div>
    );
  }

  const dot = stateDotFor(toneFor(session.lifecycle));
  const here = planeOf(session);
  /*
   * Both planes, or neither strip. `switchFor` answers whether the kernel
   * advertises the other one — which is a fact about the kernel and not about
   * the session's state, so a strip drawn from it does not appear and vanish
   * as a session starts and stops.
   */
  const other = switchFor(session, kernels);

  return (
    <div className="sessionheader">
      <span className="sessionheader__crumb" title={session.kernel}>
        {tabLabel(session.createdAt)}
      </span>

      {/* The state as a dot and a word — colour is never the only signal. */}
      {waiting ? (
        <Pill>
          <StateDot state="warning" />
          Waiting for you
        </Pill>
      ) : (
        <Pill>
          {dot === undefined ? null : <StateDot state={dot} />}
          {labelFor(session.lifecycle)}
        </Pill>
      )}

      {other === undefined || onChangePlane === undefined ? null : (
        <div className="sessionheader__tabs" role="tablist" aria-label="Plane">
          {[
            { id: here, label: labelForPlane(here), current: true },
            { id: other.to, label: other.label, current: false },
          ].map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              className="sessionheader__tab"
              aria-selected={tab.current}
              /*
               * The tab that is already showing is never disabled: pressing
               * it is a no-op, and disabling the current tab is how a strip
               * ends up with nothing selectable on it.
               */
              aria-disabled={!tab.current && !other.available}
              title={tab.current || other.available ? undefined : unavailableBecause(session)}
              onClick={() => {
                if (tab.current || !other.available) {
                  return;
                }
                onChangePlane(other.to);
              }}
            >
              {tab.label}
            </button>
          ))}
        </div>
      )}

      <span className="sessionheader__spacer" />
      {actions}
    </div>
  );
}

/** What a plane is called, from the one list that names them. */
function labelForPlane(plane: SessionPlane): string {
  return PLANES.find((entry) => entry.id === plane)?.label ?? plane;
}
