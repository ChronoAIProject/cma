import { useEffect, useState } from 'react';
import type { Session, SessionCreator, SessionService } from './types';
import { Pill } from '../ui/Pill';
import { abilitiesFor } from './availability';
import { labelFor } from './types';
import { fullTimestamp, fullTimestampOrNone } from './sessionTime';
import './SessionDrawer.css';
import { IconCloseOutline16 } from '../ui/icons';

/**
 * What is known about the open session — docs/Web_Style.md §12.
 *
 * A right drawer, 360px, closed by default and opened from the header's
 * Details key. It is reference material: read when somebody wants to know
 * which agent is answering or why a session stopped. The previous revision's
 * 412px bottom sheet took that height from the surface it was describing —
 * on a 900px window, a third of the terminal — where the drawer takes width
 * from a surface that has a thousand pixels of it and none of the height.
 *
 * Below 75rem it overlays the surface rather than narrowing it (§16).
 *
 * At its foot, away from everything reversible, sit **Pause session** and
 * **Delete** — the two acts that release or destroy, kept out of the header
 * so reaching for Details never lands on one.
 */

interface SessionDrawerProps {
  readonly session: Session;
  readonly onClose: () => void;
  /** Why the session ended, when it ended badly. */
  readonly ending?: string;
}

export function SessionDrawer({ session, onClose, ending }: SessionDrawerProps) {
  const can = abilitiesFor(session);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = window.setTimeout(() => {
      setCopied(false);
    }, 2000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [copied]);

  const copyIdentifier = () => {
    /*
     * Clipboard access needs a secure context and can be refused. The control
     * only claims "Copied" after the write resolved — claiming it before
     * would report an action that did not happen.
     */
    void navigator.clipboard
      ?.writeText(session.id)
      .then(() => {
        setCopied(true);
      })
      .catch(() => {
        // Refused by the browser. The identifier is still selectable text.
      });
  };

  return (
    <aside className="drawer" aria-label="Session details">
      {/*
       * No title. The panel is named to assistive technology by the `aside`'s
       * own label, and a heading reading "Details" over a list of details told
       * a sighted reader what they were already looking at — spending the top
       * of a 360px column on it, so the first fact sat a row lower than it
       * needed to.
       *
       * The close key floats over the corner rather than sitting in a band of
       * its own, for the same reason: a band exists to hold a title.
       */}
      <button
        type="button"
        className="drawer__close"
        aria-label="Close session details"
        onClick={onClose}
      >
        <IconCloseOutline16 size={14} />
      </button>

      <div className="drawer__body">
        <dl className="drawer__list">
          {/*
           * Creator first, because it answers "whose is this" — the question
           * somebody reading another person's session opens this drawer to
           * ask. Omitted entirely when the control plane did not name them,
           * rather than drawn as a row with nothing in it.
           */}
          {session.creator === undefined ? null : (
            <div className="drawer__row">
              <dt>Creator</dt>
              <dd className="drawer__creator">
                <CreatorFace creator={session.creator} />
                <span>{session.creator.displayName ?? 'Not named'}</span>
              </dd>
            </div>
          )}
          <div className="drawer__row">
            <dt>Started</dt>
            <dd>{fullTimestamp(session.createdAt)}</dd>
          </div>
          <div className="drawer__row">
            <dt>Agent</dt>
            <dd>{session.kernel}</dd>
          </div>
          <div className="drawer__row">
            <dt>State</dt>
            <dd>{labelFor(session.lifecycle)}</dd>
          </div>
          {/*
           * Only present once a session has been moved between runtimes.
           * Shown because the alternative is someone noticing a gap in their
           * terminal and having no way to find out what caused it.
           */}
          {session.runtimeVersion !== undefined ? (
            <div className="drawer__row">
              <dt>Runtime</dt>
              <dd>{session.runtimeVersion}</dd>
            </div>
          ) : null}
          {session.rotations !== undefined && session.rotations > 0 ? (
            <div className="drawer__row">
              <dt>Moved</dt>
              <dd>
                {session.rotations === 1 ? 'once' : `${String(session.rotations)} times`}
                {session.rotatedAt !== undefined
                  ? `, last on ${fullTimestampOrNone(session.rotatedAt)}`
                  : ''}
              </dd>
            </div>
          ) : null}
          {/*
           * What this session can touch, beside what it can do. §11 and §12:
           * an owner watching an agent work on their behalf has one question
           * the interface did not answer — what does this thing have access
           * to right now?
           *
           * The session's answer and never its profile's. A profile can be
           * edited after a session started from it and the session keeps the
           * scope it was provisioned with, so only the session knows.
           */}
          <div className="drawer__row">
            <dt>Reaches</dt>
            <dd>
              <Reach services={session.reach} />
            </dd>
          </div>
          <div className="drawer__row">
            <dt>Identifier</dt>
            <dd>
              {/*
               * Copyable, because it is what support and logs refer to, and a
               * partial identifier is not one anyone can use.
               */}
              <button type="button" className="drawer__id" onClick={copyIdentifier}>
                <span className="drawer__idvalue">{session.id}</span>
                <span className="drawer__idhint">{copied ? 'Copied' : 'Copy'}</span>
              </button>
            </dd>
          </div>
        </dl>

        {/*
         * The reason the session cannot be typed into, stated wherever it is
         * true rather than only when something has gone wrong. A terminal
         * that ignores keystrokes without saying why is the thing people
         * report as broken.
         */}
        {can.reason !== undefined ? <p className="drawer__state">{can.reason}</p> : null}

        {ending !== undefined && ending !== '' ? <p className="drawer__ending">{ending}</p> : null}
      </div>

      {/*
       * No foot. Every lifecycle act is one key in the session header now, and
       * splitting them across two surfaces is how somebody comes looking for
       * an action where it used to be. Delete moved with the rest, keeping the
       * acknowledgement §12.4 spends on the one action that loses something.
       *
       * This panel is reference material, and reference material that also
       * acts is a panel people are afraid to open.
       */}
    </aside>
  );
}

/**
 * What a session's credential may reach.
 *
 * Three answers, drawn as three things. A list is what it was granted. An
 * empty list is a session that reaches nothing, said plainly — an empty list
 * and an absent mechanism must not look alike. And a control plane that did
 * not report it at all says so, rather than being drawn as either.
 */
function Reach({ services }: { readonly services?: readonly SessionService[] }) {
  if (services === undefined) {
    return <span className="drawer__unknown">Not reported by the control plane</span>;
  }
  if (services.length === 0) {
    return <span className="drawer__unknown">Nothing — this session reaches no services</span>;
  }
  return (
    <ul className="drawer__reach">
      {services.map((service) => (
        <li className="drawer__service" key={service.id}>
          {/*
           * The name where the catalogue offered one, and the identifier
           * always: a catalogue that could not be asked costs the name and
           * not the answer, and an identifier alone is still true.
           */}
          <span className="drawer__servicename">{service.name ?? service.id}</span>
          {service.name === undefined ? null : (
            <span className="drawer__serviceid">{service.id}</span>
          )}
          {/*
           * Mandated entries are drawn as fixed, matching the picker's
           * existing treatment and for the same stated reason: a person
           * deciding what a session may touch should see the whole answer.
           */}
          {service.mandated ? <Pill>always</Pill> : null}
        </li>
      ))}
    </ul>
  );
}

/**
 * The creator's picture, or their initial when there is none.
 *
 * A picture that fails to load falls back the same way rather than leaving a
 * broken image in a panel somebody is reading to identify a session.
 */
function CreatorFace({ creator }: { readonly creator: SessionCreator }) {
  const [broken, setBroken] = useState(false);
  const initial = creator.displayName?.trim().charAt(0).toUpperCase();
  if (creator.avatarUrl === undefined || broken) {
    return (
      <span className="drawer__face" aria-hidden="true">
        {initial === undefined || initial === '' ? '?' : initial}
      </span>
    );
  }
  return (
    <img
      className="drawer__face"
      src={creator.avatarUrl}
      alt=""
      onError={() => {
        setBroken(true);
      }}
    />
  );
}
