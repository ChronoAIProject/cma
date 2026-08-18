import { useEffect, useRef, useState } from 'react';
import { AgentContent } from './AgentContent';
import { ToolCall } from './ToolCall';
import type { Entry } from './conversation';
import { fullTimestampOrNone } from '../sessions/sessionTime';
import './MessageStream.css';

/**
 * The turn-by-turn record of the conversation — `Web_Style.md` §18.1.
 *
 * Turns, not bubbles: a band of content separated from the one before it by a
 * rule, labelled with who is speaking, and inked so the agent's words are the
 * ones being read.
 *
 * **Nothing about a message changes when it commits.** A message that
 * restyled itself on commit would tell a reader the earlier state was
 * provisional when it was not; the only difference is the caret, which the
 * turn in flight carries and a settled one does not.
 *
 * The surface is pinned to the newest turn while the newest turn is the one
 * growing, and stops being pinned the moment somebody scrolls up — a person
 * reading something from four turns ago has said what they want. It does not
 * scroll back down on its own; a quiet control says how far behind they are.
 */

interface MessageStreamProps {
  readonly entries: readonly Entry[];
  /** Drawn under the last entry while a turn is in flight. */
  readonly turnInFlight?: boolean;
  /** Rendered at the foot of the stream — the approval card, when there is one. */
  readonly footer?: React.ReactNode;
}

/** How close to the bottom still counts as "following the conversation". */
const PINNED_WITHIN_PX = 48;

export function MessageStream({ entries, turnInFlight = false, footer }: MessageStreamProps) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  /** How much had arrived the last time somebody was looking at the bottom. */
  const [seenAt, setSeenAt] = useState(entries.length);

  /*
   * Derived during render rather than set from the effect below. It is a
   * function of what has arrived and where the reader is, and computing it in
   * an effect would mean a second render to reach the answer the first one
   * already had.
   */
  const behind = pinned ? 0 : Math.max(0, entries.length - seenAt);

  useEffect(() => {
    // The effect does the one thing effects are for: telling an external
    // system — the scroll position — what React already decided.
    const surface = surfaceRef.current;
    if (surface !== null && pinned) {
      surface.scrollTop = surface.scrollHeight;
    }
  }, [entries, pinned]);

  return (
    <div className="stream">
      <div
        className="stream__surface"
        ref={surfaceRef}
        role="log"
        aria-label="Conversation"
        aria-live="polite"
        onScroll={(event) => {
          const surface = event.currentTarget;
          const atBottom =
            surface.scrollHeight - surface.scrollTop - surface.clientHeight <= PINNED_WITHIN_PX;
          setPinned(atBottom);
          /*
           * Recorded on every scroll, not only on reaching the bottom. While
           * somebody is following along `behind` is zero whatever this holds;
           * the moment they scroll away, what they have seen is exactly what
           * had arrived by then, and this is the event that knows it.
           */
          setSeenAt(entries.length);
        }}
      >
        {entries.map((entry) => (
          <StreamEntry
            entry={entry}
            key={`${entry.kind}:${entry.id}`}
            turnInFlight={turnInFlight}
          />
        ))}
        {footer}
      </div>

      {/*
       * §18.1: it does not scroll back down on its own. A person reading
       * something the agent said four turns ago has said what they want, so
       * what arrives while they read is announced rather than chased.
       */}
      {behind > 0 ? (
        <button
          type="button"
          className="button button--sm press--quiet stream__behind"
          onClick={() => {
            setPinned(true);
          }}
        >
          {behind} newer {behind === 1 ? 'entry' : 'entries'}
        </button>
      ) : null}
    </div>
  );
}

function StreamEntry({ entry, turnInFlight }: { entry: Entry; turnInFlight: boolean }) {
  switch (entry.kind) {
    case 'message':
      return (
        <article className="turn" data-role={entry.role}>
          <span className="turn__who">{entry.role === 'user' ? 'You' : 'Agent'}</span>
          <div className="turn__body">
            <AgentContent text={entry.text} />
            {/*
             * The one difference between arriving and settled. Same ink, same
             * size, same everything else.
             */}
            {!entry.committed && turnInFlight ? (
              <b className="turn__caret" aria-hidden="true" />
            ) : null}
          </div>
        </article>
      );

    case 'gap':
      /*
       * A boundary drawn where history stops being continuous — §18.6 and
       * CLAUDE.md §5. Not a status chip: an owner who can see the boundary
       * recognises a gap rather than puzzling over one, and one they cannot
       * see reads as the agent having forgotten.
       */
      return (
        <div className="gap" role="separator">
          <span className="gap__label">
            {entry.movedTo === undefined
              ? 'Carried forward from '
              : `Moved to ${entry.movedTo}, carried forward from `}
            {fullTimestampOrNone(Date.parse(entry.rotatedAt ?? ''))}
          </span>
          <span className="gap__note">Anything after that moment did not travel.</span>
        </div>
      );

    case 'failure':
      /*
       * §18.9. Drawn in the stream at the point it stopped, never in place of
       * the surface: what the agent had already done stays readable, and a
       * failure hoisted into a band loses the one thing that says what it was
       * doing when it stopped.
       *
       * The word carries the meaning and the ink only reinforces it — §2
       * forbids colour alone. The capability is named where there is a name,
       * because "something went wrong" is the one part of a refusal nobody
       * can act on.
       */
      return (
        <div className="failure" role="status">
          <span className="failure__label">Run failed</span>
          <div className="failure__body">
            <p className="failure__message">{entry.message}</p>
            {entry.capability === undefined ? null : (
              <p className="failure__named">
                This session cannot <b>{entry.capability}</b>.
              </p>
            )}
            {entry.code === undefined ? null : (
              <span className="failure__code">{entry.code}</span>
            )}
          </div>
        </div>
      );

    case 'unknown':
      /*
       * Shown rather than dropped. The emitted set grows additively, and a
       * client that hid what it did not recognise would leave somebody
       * looking at a conversation with a piece silently missing.
       */
      return (
        <div className="turn turn--unknown">
          <span className="turn__who">{entry.eventKind}</span>
          <span className="turn__body">
            This build does not know how to draw this yet. It was not lost.
          </span>
        </div>
      );

    case 'tool':
      /*
       * §18.2: structure rather than prose. It sits in the stream where it
       * happened, between the turns it happened between, because that is
       * where a reader is looking for what the agent did next.
       */
      return (
        <div className="turn turn--tool">
          <ToolCall call={entry} />
        </div>
      );

    default: {
      const exhaustive: never = entry;
      return exhaustive;
    }
  }
}
