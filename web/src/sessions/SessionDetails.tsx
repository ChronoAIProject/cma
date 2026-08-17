import { useState } from 'react';
import type { Session, SessionCreator } from './types';
import { abilitiesFor } from './availability';
import { labelFor } from './types';
import { fullTimestamp, fullTimestampOrNone } from './sessionTime';
import './SessionDetails.css';

/**
 * What is known about the open session — docs/Web_Style.md §12.
 *
 * A sheet under the terminal, not a column beside it. It is reference
 * material: read when somebody wants to know which agent is answering or why a
 * session stopped, and the rest of the time it is width the terminal should
 * have. Open it is 412px and its facts sit wide rather than tall; closed it is
 * a 40px band carrying a one-line summary, because a collapsed panel is a
 * handle and never nothing (§7).
 *
 * Its columns scroll inside it. The page does not scroll, and neither does the
 * terminal above it: a sheet that grew with its contents would take the height
 * from the surface it is describing.
 *
 * It carries no lifecycle controls. §12 puts those in the session header, at
 * the head of the terminal they act on, visible without opening anything.
 */

interface SessionDetailsProps {
  readonly session: Session;
  readonly open: boolean;
  readonly onToggle: () => void;
  /** Why the session ended, when it ended badly. */
  readonly ending?: string;
}

export function SessionDetails({ session, open, onToggle, ending }: SessionDetailsProps) {
  const reason = abilitiesFor(session).reason;

  const handle = (
    <button
      type="button"
      className="collapse-handle details__handle"
      onClick={onToggle}
      aria-expanded={open}
      aria-label={open ? 'Hide session details' : 'Show session details'}
    >
      {/*
       * The glyph points where the sheet will go and reverses when collapsed,
       * so the control describes its outcome rather than its state (§7).
       */}
      <span aria-hidden="true">{open ? '⌄' : '⌃'}</span>
    </button>
  );

  if (!open) {
    return (
      <aside className="band band--foot details details--closed" aria-label="Session details">
        {/*
         * The one-line summary of what the sheet hides. Collapsing gives up
         * detail, not awareness, so the facts that decide what a person can do
         * with this session — which agent, what state, and why it cannot be
         * typed into — are the ones that stay.
         */}
        <span className="band__label">Session</span>
        <span className="band__value">{session.kernel}</span>
        <span className="band__value">{labelFor(session.lifecycle)}</span>
        {reason === undefined ? null : (
          <span className="band__value details__summaryreason">{reason}</span>
        )}
        <span className="band__spacer" />
        {handle}
      </aside>
    );
  }

  return (
    <aside className="details" aria-label="Session details">
      <div className="band band--foot details__head">
        <span className="band__label">Session</span>
        <span className="band__spacer" />
        {handle}
      </div>

      {/*
       * The facts sit wide rather than tall — §12. A 412px sheet running one
       * column deep would scroll for something that fits across.
       */}
      <div className="details__body">
        <dl className="details__list">
          {/*
           * First, because it answers "whose is this" — the question somebody
           * reading another person's session opens this sheet to ask. Omitted
           * entirely when the control plane did not name them, rather than
           * drawn as a row with nothing in it.
           */}
          {session.creator === undefined ? null : (
            <div className="details__row">
              <dt>Creator</dt>
              <dd className="details__creator">
                <CreatorFace creator={session.creator} />
                <span>{session.creator.displayName ?? 'Not named'}</span>
              </dd>
            </div>
          )}
          <div className="details__row">
            <dt>Started</dt>
            <dd>{fullTimestamp(session.createdAt)}</dd>
          </div>
          <div className="details__row">
            <dt>Agent</dt>
            <dd>{session.kernel}</dd>
          </div>
          <div className="details__row">
            <dt>State</dt>
            <dd>{labelFor(session.lifecycle)}</dd>
          </div>
          {/*
           * Only present once a session has been moved between runtimes. Shown
           * because the alternative is someone noticing a gap in their terminal
           * and having no way to find out what caused it.
           */}
          {session.runtimeVersion !== undefined ? (
            <div className="details__row">
              <dt>Runtime</dt>
              <dd>{session.runtimeVersion}</dd>
            </div>
          ) : null}
          {session.rotations !== undefined && session.rotations > 0 ? (
            <div className="details__row">
              <dt>Moved</dt>
              <dd>
                {session.rotations === 1 ? 'once' : `${String(session.rotations)} times`}
                {session.rotatedAt !== undefined
                  ? `, last on ${fullTimestampOrNone(session.rotatedAt)}`
                  : ''}
              </dd>
            </div>
          ) : null}
          <div className="details__row">
            <dt>Identifier</dt>
            {/*
             * Shown because it is what support and logs refer to, and wrapped
             * rather than truncated: a partial identifier is not one anyone can
             * use.
             */}
            <dd className="details__id">{session.id}</dd>
          </div>
        </dl>

        {/*
         * The reason the session cannot be typed into, stated wherever it is
         * true rather than only when something has gone wrong. A terminal that
         * ignores keystrokes without saying why is the thing people report as
         * broken.
         */}
        {reason !== undefined ? <p className="details__state">{reason}</p> : null}

        {ending !== undefined && ending !== '' ? <p className="details__ending">{ending}</p> : null}
      </div>
    </aside>
  );
}

/**
 * The creator's picture, or their initial when there is none.
 *
 * A picture that fails to load falls back the same way rather than leaving a
 * broken image in a sheet somebody is reading to identify a session.
 */
function CreatorFace({ creator }: { readonly creator: SessionCreator }) {
  const [broken, setBroken] = useState(false);
  const initial = creator.displayName?.trim().charAt(0).toUpperCase();
  if (creator.avatarUrl === undefined || broken) {
    return (
      <span className="details__face" aria-hidden="true">
        {initial === undefined || initial === '' ? '?' : initial}
      </span>
    );
  }
  return (
    <img
      className="details__face"
      src={creator.avatarUrl}
      alt=""
      onError={() => {
        setBroken(true);
      }}
    />
  );
}
