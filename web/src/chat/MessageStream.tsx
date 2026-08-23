import { useEffect, useRef, useState } from 'react';
import { AgentContent } from './AgentContent';
import { CopyKey } from './CopyKey';
import { Thinking } from './Thinking';
import { ToolCall } from './ToolCall';
import { formatTokens } from './Usage';
import type { ConfigOption, Entry, MessageEntry, PlanStep, ToolCallEntry } from './conversation';
import { fullTimestampOrNone, timeOfDay } from '../sessions/sessionTime';
import './MessageStream.css';
import { IconChevronDownOutline14 } from '../ui/icons';
import { StateDot } from '../ui/StateDot';

/**
 * The turn-by-turn record of the conversation — `Web_Style.md` §18.1.
 *
 * A reading column, 760px, centred. The two roles are told apart by shape as
 * well as ink: the agent's turn is full width and unboxed under its eyebrow,
 * at the reading size, because the agent's words are the ones being read and
 * they get the whole column; the owner's turn is a right-aligned block on
 * the inset, because it is short, it is already known, and a shape means the
 * eye finds where a request starts without reading a label.
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
  /**
   * Open a tool call in the details column.
   *
   * Absent where there is no column — a row is then a summary and says so by
   * not being pressable, rather than by opening onto nothing.
   */
  readonly onInspect?: (call: ToolCallEntry) => void;
  /** The call the column is showing, when it is showing one. */
  readonly inspectingId?: string;
  /** Drawn under the last entry while a turn is in flight. */
  readonly turnInFlight?: boolean;
  /**
   * Whether the surface can currently hear the session — §18.13.
   *
   * While this is false the surface stops claiming the agent is working: no
   * pulse, no caret. Nothing but the run ending used to clear the in-flight
   * state, so a dead stream kept both going on a message that had stopped
   * growing — the one affordance built to tell a working agent from a dead
   * stream was the one that lied.
   */
  readonly linkLive?: boolean;
  /**
   * Whether the link has stayed down long enough to be worth naming — §18.13.
   *
   * A transient reconnect says nothing. This is the bounded threshold, decided
   * by the surface that owns the connection rather than counted here.
   */
  readonly linkLost?: boolean;
  /**
   * What the agent is saying right now, before it has committed to it.
   *
   * Drawn after the last entry and never as one — §18.1. It is *replaced* by
   * the committed message rather than appended to, so a reload, which has no
   * live message at all, draws the same conversation.
   */
  readonly live?: { readonly text: string };
  /**
   * What the agent is reasoning about, while it reasons — §18.8.
   *
   * Drawn above the in-flight message, because that is the order it happens
   * in. Nothing is drawn when there is none: §18.4's rule that a control which
   * can never work is not one currently unavailable applies here too, and an
   * empty band would suggest a kernel was thinking silently.
   */
  readonly reasoning?: { readonly text: string };
  /**
   * The agent's plan, when it set one — drawn as a stage rather than as prose.
   *
   * A full revision, so the list shown is the whole current plan and never an
   * accumulation of older ones.
   */
  readonly plan?: readonly PlanStep[];
  /** The session's permission mode, when the kernel changed it. */
  readonly mode?: string;
  /** The settings the kernel says are settable, when it reported them. */
  readonly configOptions?: readonly ConfigOption[];
  /** The actions the owner may invoke right now — liveness, not history. */
  /** Rendered at the foot of the stream — the approval card, when there is one. */
  readonly footer?: React.ReactNode;
}

/** How close to the bottom still counts as "following the conversation". */
const PINNED_WITHIN_PX = 48;

export function MessageStream({
  entries,
  onInspect,
  inspectingId,
  turnInFlight = false,
  linkLive = true,
  linkLost = false,
  live,
  reasoning,
  footer,
  plan,
  mode,
  configOptions,
}: MessageStreamProps) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const columnRef = useRef<HTMLDivElement>(null);
  /* Read inside the growth observer, which outlives any one render. */
  const pinnedRef = useRef(true);
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

  /*
   * §18.1: between the turn starting and the first thing arriving, the wave
   * with a word. A turn that has started and produced nothing is otherwise
   * indistinguishable from a dead stream, and a turn takes as long as the
   * agent takes. Removed the moment anything arrives, and claiming no
   * progress — §11's reason for a wave rather than a bar.
   */
  /*
   * The claim is withdrawn the moment the link is not live — §18.13. A pulse
   * that says "working" while nothing is arriving is worse than drawing
   * nothing, because it is the affordance a reader trusts to tell them the
   * difference.
   */
  const waiting = turnInFlight && linkLive && live === undefined && reasoning === undefined;

  /*
   * Follow the column while it grows, for as long as somebody is following.
   *
   * Observed rather than listed. This depended on `entries`, and the message
   * being typed is deliberately not an entry (§18.1) — a streaming delta
   * lands in `live` and returns a conversation holding the *same* entries
   * array, so the effect never ran and the view stopped following at exactly
   * the moment text started arriving. Reasoning, the approval card and an
   * opened thinking band were invisible to it for the same reason.
   *
   * A dependency list would have to name every one of those and would rot the
   * next time the surface grows something. Growth is the actual condition, so
   * growth is what is watched.
   */
  useEffect(() => {
    const surface = surfaceRef.current;
    const column = columnRef.current;
    if (surface === null || column === null || typeof ResizeObserver === 'undefined') {
      return;
    }
    const follow = () => {
      // Read at the moment it fires: `pinned` is state, and a stale closure
      // would keep dragging somebody back who has scrolled away.
      if (pinnedRef.current) {
        surface.scrollTop = surface.scrollHeight;
      }
    };
    follow();
    const observer = new ResizeObserver(follow);
    observer.observe(column);
    return () => {
      observer.disconnect();
    };
  }, []);

  // Pinning again jumps to the newest turn at once, rather than waiting for
  // the next thing to arrive.
  useEffect(() => {
    pinnedRef.current = pinned;
    const surface = surfaceRef.current;
    if (surface !== null && pinned) {
      surface.scrollTop = surface.scrollHeight;
    }
  }, [pinned]);

  return (
    <div className="stream">
      <div
        className="stream__surface"
        ref={surfaceRef}
        role="log"
        aria-label="Conversation"
        aria-live="polite"
        /*
         * Announce turns as they arrive, not characters as they are typed.
         *
         * The default for a live region includes text changes inside nodes it
         * already holds, and the agent's message grows one delta at a time —
         * so a screen reader was re-reading the message from the top on every
         * token, which for a long answer never finishes and cannot be
         * interrupted. Restricted to additions, the reader hears each turn
         * once, when it lands.
         */
        aria-relevant="additions"
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
        <div className="stream__column" ref={columnRef}>
          {/*
           * A session that has said nothing is not a broken one, and a blank
           * column does not say so. It said nothing at all: the surface came
           * up empty and the only clue that it was ready was that the composer
           * had stopped refusing.
           *
           * Drawn only where there is genuinely nothing — the moment a turn
           * exists this is gone, and it never sits above a conversation.
           *
           * `linkLive` is part of the condition and not decoration. An empty
           * column means "this session has said nothing" only once the log has
           * actually been read; before that it means "nothing has arrived
           * yet", and those are different facts. Without it, opening a session
           * with a long history announced "Ready when you are." over its own
           * conversation until the replay landed — telling somebody their work
           * is not there, in the moment they are waiting to see it.
           */}
          {entries.length === 0 && plan === undefined && linkLive ? (
            <div className="stream__opening">
              <p className="stream__openingtitle">Ready when you are.</p>
              <p className="stream__openinghint">
                Say what you want done, and this session will get to work. Press{' '}
                <kbd className="stream__key">/</kbd> for what it can do.
              </p>
            </div>
          ) : null}
          {entries.map((entry) => (
            <StreamEntry
              entry={entry}
              key={`${entry.kind}:${entry.id}`}
              turnInFlight={turnInFlight}
              linkLive={linkLive}
              onInspect={onInspect}
              inspectingId={inspectingId}
            />
          ))}
          {plan === undefined || plan.length === 0 ? null : (
            <ol className="stage">
              {plan.map((step, index) => (
                <li key={index} className="stage__step">
                  <span className="stage__title">{step.title}</span>
                  <span className="stage__status">{step.status}</span>
                </li>
              ))}
            </ol>
          )}
          {mode === undefined ? null : <div className="stage__mode">Mode: {mode}</div>}
          {configOptions === undefined || configOptions.length === 0 ? null : (
            <ul className="stage">
              {configOptions.map((option) => (
                <li key={option.id} className="stage__step">
                  <span className="stage__title">{option.name}</span>
                  <span className="stage__status">{option.kind}</span>
                </li>
              ))}
            </ul>
          )}
          {reasoning === undefined ? null : <Thinking text={reasoning.text} />}
          {live === undefined ? null : (
            /*
             * The in-flight message. Same shape, same ink, same everything as
             * a committed one — §18.1 forbids a message restyling itself when
             * it settles, and this is the same rule read forwards. The caret
             * is the one difference, and it belongs to this and nothing else.
             */
            /*
             * Silent while it is being typed. The committed message that
             * replaces it is an addition and is announced then, once and
             * whole — §18.1's equivalence rule read for a listener: what is
             * heard should be the turn, not the typing.
             */
            <article className="turn" data-role="assistant" aria-live="off">
              <div className="turn__body">
                <AgentContent text={live.text} />
                {linkLive ? <b className="turn__caret" aria-hidden="true" /> : null}
              </div>
            </article>
          )}
          {linkLost ? (
            /*
             * §18.13, and not §18.9. A failed run is a run the agent could not
             * finish; this is a surface that cannot hear one, and conflating
             * them tells an owner their work broke when their network did — so
             * it carries no `--fail` ink and no stop reason.
             *
             * Where the turn is rather than in a band across the head: the
             * reader is looking at the turn, and §12 puts a failed *action* at
             * the head of the column, which this is not.
             */
            <p className="stream__unheard" role="status">
              Not hearing this session. What is above arrived before the connection stopped, and
              nothing has been lost — it resumes on its own.
            </p>
          ) : null}
          {waiting ? (
            /*
             * §18.1: a mark and a word, claiming no progress. The mark is the
             * running state dot the rest of the product uses — a turn that is
             * running and a session that is running are the same fact, and
             * they were drawn two different ways.
             */
            <p className="stream__waiting">
              <StateDot state="ongoing" />
              Working
            </p>
          ) : null}
          {footer}
        </div>
      </div>

      {/*
       * §18.1: it does not scroll back down on its own. A person reading
       * something the agent said four turns ago has said what they want, so
       * what arrives while they read is announced rather than chased.
       */}
      {behind > 0 ? (
        <button
          type="button"
          className="stream__behind"
          /*
           * The count is the accessible name rather than the label: a circle
           * with a chevron in it says "go down" to somebody who can see it,
           * and says nothing at all otherwise.
           */
          aria-label={`${String(behind)} newer ${behind === 1 ? 'entry' : 'entries'}. Go to the newest.`}
          onClick={() => {
            setPinned(true);
          }}
        >
          <IconChevronDownOutline14 />
        </button>
      ) : null}
    </div>
  );
}

/**
 * The owner turn's eyebrow — §18.1: `You · HH:MM`, the time read from the
 * durable log. A turn whose time the record does not carry shows none rather
 * than an estimated one, on the section's equivalence rule.
 */
function youLabel(entry: MessageEntry): string {
  if (entry.at === undefined) {
    return 'You';
  }
  const parsed = Date.parse(entry.at);
  return Number.isNaN(parsed) ? 'You' : `You · ${timeOfDay(parsed)}`;
}

function StreamEntry({
  entry,
  turnInFlight,
  linkLive,
  onInspect,
  inspectingId,
}: {
  entry: Entry;
  turnInFlight: boolean;
  /** §18.13: the caret claims the agent is typing, so it stops when we cannot hear it. */
  linkLive: boolean;
  onInspect?: ((call: ToolCallEntry) => void) | undefined;
  inspectingId?: string | undefined;
}) {
  switch (entry.kind) {
    case 'message':
      return (
        <article className="turn" data-role={entry.role}>
          {/*
            No eyebrow on an agent turn. It repeated "CMA · codex" above
            every single one, which is a label that stops being read after the
            first and takes a line of the reading column with it each time.
            Shape already tells the two roles apart — the owner's turns are
            bubbles and the agent's run the full measure — and which kernel is
            answering is a fact about the session, so the session header says
            it once.
          */}
          {entry.role === 'user' ? (
            <span className="turn__eyebrow turn__eyebrow--you">{youLabel(entry)}</span>
          ) : null}
          <div className="turn__body">
            <AgentContent text={entry.text} />
            {/*
             * The one difference between arriving and settled. Same ink, same
             * size, same everything else.
             */}
            {!entry.committed && turnInFlight && linkLive ? (
              <b className="turn__caret" aria-hidden="true" />
            ) : null}
          </div>
          {/*
           * What a reader can take away — §18.11.
           *
           * It writes `entry.text`, the markdown source the model already
           * holds, and not the rendered tree. That is the point rather than a
           * detail: selection-copy already works and already yields degraded
           * text, because the renderer strips heading hashes, list markers,
           * fences and backticks before drawing. Somebody pasting an agent's
           * numbered steps into an issue, or its backticked command into a
           * shell, gets something the agent did not write.
           *
           * Persistently drawn, never revealed on hover — §3. Disabled with a
           * reason while the turn is still being written rather than removed:
           * copying half an answer is the thing to prevent, and taking the
           * control away is the wrong instrument for it.
           */}
          <div className="turn__actions">
            <CopyKey
              text={entry.text}
              refusedReason={
                !entry.committed && turnInFlight ? 'This turn is still being written' : undefined
              }
            />
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

    case 'contextCompaction': {
      /*
       * A neutral boundary, not a warning. Both readings are the fact: one
       * number alone would leave the reader to infer what changed, while the
       * kernel exposes no signal that can say who asked for the summary.
       */
      const ceiling = entry.window === undefined ? '' : ` of ${formatTokens(entry.window)}`;
      return (
        <div className="context-compaction" role="separator">
          The agent summarised the conversation to make room. It was holding{' '}
          {formatTokens(entry.was)}; it now holds {formatTokens(entry.now)}
          {ceiling}.
        </div>
      );
    }

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
            {entry.code === undefined ? null : <span className="failure__code">{entry.code}</span>}
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
          <span className="turn__eyebrow">{entry.eventKind}</span>
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
          <ToolCall call={entry} onInspect={onInspect} inspecting={entry.id === inspectingId} />
        </div>
      );

    default: {
      const exhaustive: never = entry;
      return exhaustive;
    }
  }
}
