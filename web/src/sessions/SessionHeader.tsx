import type { ReactNode } from 'react';
import type { Session } from './types';
import { labelFor, stateDotFor, toneFor } from './types';
import { Pill } from '../ui/Pill';
import { StateDot } from '../ui/StateDot';
import { tabLabel } from './sessionTime';
import './SessionHeader.css';

/**
 * The centre column's own header — `docs/Web_Style.md` §11.
 *
 * Not chrome. It belongs to the column beneath it and scrolls nothing: the
 * crumb says which session is showing and the trailing cluster holds what acts
 * on it.
 *
 * The controls are here rather than in the details column because §12 puts
 * them "at the head of the surface they act on, visible without opening
 * anything". In a panel they were reachable only by opening reference
 * material, so ending a session meant first opening something nobody wanted
 * to read.
 *
 * **It carries no plane strip.** A session is a conversation; the terminal is
 * something its owner deliberately moves it to, from the actions menu, and a
 * pair of tabs made a rotation look like a view switch — free, instant, and
 * reversible, which it is none of.
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
  /** What an intentionally empty seat is called. */
  readonly emptyLabel?: string;
}

export function SessionHeader({
  session,
  waiting = false,
  actions,
  emptyLabel = 'No session open',
}: SessionHeaderProps) {
  if (session === null) {
    return (
      <div className="sessionheader">
        <span className="sessionheader__crumb">{emptyLabel}</span>
      </div>
    );
  }

  const dot = stateDotFor(toneFor(session.lifecycle));
  const chat = session.chat;
  return (
    <div className="sessionheader" data-surface={chat === undefined ? 'agent' : 'chat'}>
      <span className="sessionheader__crumb">
        {chat === undefined ? tabLabel(session.createdAt) : (chat.title ?? chat.worktreeName)}
      </span>
      {/*
        Which agent is answering, said once. It used to ride an eyebrow above
        every agent turn, where it repeated identically down the whole column
        and took a line of the reading measure each time. It is a fact about
        the session, so it belongs with the session.
      */}
      {chat === undefined ? <span className="sessionheader__kernel">{session.kernel}</span> : null}

      {/* The state as a dot and a word — colour is never the only signal. */}
      {waiting ? (
        <Pill>
          <StateDot state="warning" />
          Waiting for you
        </Pill>
      ) : (
        <Pill>
          {dot === undefined ? null : <StateDot state={dot} />}
          {labelFor(session.lifecycle, session.reportedStatus)}
        </Pill>
      )}

      <span className="sessionheader__spacer" />
      {actions}
    </div>
  );
}

/** What a plane is called, from the one list that names them. */
