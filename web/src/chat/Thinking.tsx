import { useEffect, useRef, useState } from 'react';
import './Thinking.css';
import { IconThinkOutline14 } from '../ui/icons';

/**
 * What the agent is reasoning about, while it reasons — `Web_Style.md` §18.8.
 *
 * **It is liveness and it is never in the record.** It is removed when the
 * turn ends and a reload shows none of it. That is stated on the control
 * whether it is open or not, because a reader who saw reasoning and reloaded
 * to find it gone would otherwise report data loss.
 *
 * **Collapsed by default** to one row. The previous revision drew an
 * always-open band, which put the least durable content on the surface at the
 * same weight as the conversation. Opening it shows a height-capped band
 * pinned to the newest.
 *
 * The row carries the newest line while it is collapsed. That is the whole
 * reason to look at a shut row: "it is doing something" is what a spinner
 * says, and this says what.
 *
 * There is deliberately no settled summary. The design this follows shows the
 * first line once reasoning has stopped — but reasoning here never stops on
 * screen: §18.8 makes it liveness, so the run ending removes it entirely and
 * a row showing a finished summary is a row that cannot exist.
 *
 * No colour, ever. §2 reserves the semantic inks for session and system
 * state, and reasoning is neither — it says nothing about how the session is
 * doing.
 */

export interface ThinkingActivityBody {
  readonly text: string;
  readonly truncated: boolean;
}

/** One durable or live tool line inside a turn's Thinking disclosure. */
export interface ThinkingActivity {
  readonly id: string;
  readonly kind: string;
  readonly target: string;
  readonly status: string;
  readonly durationMs?: number | null;
  readonly arguments?: ThinkingActivityBody | null;
  readonly result?: ThinkingActivityBody | null;
}

interface ThinkingProps {
  /** Live-only reasoning. Empty for a fold reconstructed from durable tools. */
  readonly text?: string;
  readonly activities?: readonly ThinkingActivity[];
  /** Settled folds do not animate or continue counting. */
  readonly live?: boolean;
}

/** The newest line, which is what the agent is working through right now. */
export function summaryOf(text: string): string {
  const lines = text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  return lines[lines.length - 1] ?? '';
}

/** The duration column of one quiet tool-activity line. */
export function activityDuration(activity: ThinkingActivity): string {
  const duration = activity.durationMs;
  if (duration === undefined || duration === null) {
    return ['started', 'running', 'in_progress'].includes(activity.status.toLowerCase())
      ? 'ongoing'
      : 'not reported';
  }
  if (duration < 1000) {
    return `${String(duration)}ms`;
  }
  const seconds = duration / 1000;
  return `${Number.isInteger(seconds) ? String(seconds) : seconds.toFixed(1)}s`;
}

/** The fixed-order line used both in the shut-row summary and the open body. */
export function activitySummary(activity: ThinkingActivity): string {
  return `${activity.kind} - ${activity.target} - ${activity.status} - ${activityDuration(activity)}`;
}

export function Thinking({ text = '', activities = [], live = true }: ThinkingProps) {
  const [open, setOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  /*
   * How long this reasoning has been arriving, counted from mount. Liveness
   * about liveness: the figure is not kept either, and a reload starts from
   * nothing because the reasoning it measured is gone.
   */
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    if (!live) {
      return;
    }
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, [live]);

  useEffect(() => {
    // Always pinned, unlike the message stream. Nobody scrolls back through
    // reasoning — it is going to be gone in a moment — so the newest is the
    // only part worth showing.
    const body = bodyRef.current;
    if (body !== null && open) {
      body.scrollTop = body.scrollHeight;
    }
  }, [activities, text, open]);

  const recordedDuration = activities.reduce<number | undefined>((longest, activity) => {
    const duration = activity.durationMs;
    if (duration === undefined || duration === null) return longest;
    return longest === undefined ? duration : Math.max(longest, duration);
  }, undefined);
  const title =
    live || recordedDuration !== undefined
      ? `Thought for ${String(live ? seconds : Math.ceil((recordedDuration ?? 0) / 1000))}s`
      : 'Thinking';
  const activity = activities.at(-1);
  const summary = summaryOf(text) || (activity === undefined ? '' : activitySummary(activity));

  return (
    <section
      className={`thinking${live ? '' : ' thinking--settled'}`}
      aria-label="What the agent is working through"
    >
      {/*
       * The words that make it safe to lose stay on the key whether it is
       * open or not — §18.8.
       */}
      <button
        type="button"
        className="thinking__key"
        aria-expanded={open}
        onClick={() => {
          setOpen((wasOpen) => !wasOpen);
        }}
      >
        <span className="thinking__glyph" aria-hidden="true">
          <IconThinkOutline14 />
        </span>
        <span className="thinking__title">{title}</span>
        <span className="thinking__dot" aria-hidden="true" />
        {/*
          The summary is the row's reason to exist while it is shut. It never
          wraps: a row that grows with the agent's longest sentence moves the
          conversation under it several times a second.
        */}
        <span className="thinking__summary">{summary}</span>
        <span className="sr-only">
          {activities.length === 0
            ? ', not kept after this turn'
            : text === ''
              ? ', tool activity retained in this conversation'
              : ', reasoning not kept after this turn; tool activity retained in this conversation'}
        </span>
      </button>

      {/*
       * Not a live region. It changes many times a second, and announcing
       * each change would make the surface unusable with a screen reader; the
       * message that follows is what is worth hearing, and it is announced.
       */}
      {open ? (
        <div className="thinking__body" ref={bodyRef}>
          {text === '' ? null : <div className="thinking__reasoning">{text}</div>}
          {activities.length === 0 ? null : (
            <ul className="thinking__activities">
              {activities.map((item) => (
                <li className="thinking__activity" key={item.id}>
                  <p className="thinking__activityline">{activitySummary(item)}</p>
                  <ActivityBody label="Arguments" body={item.arguments} />
                  <ActivityBody label="Result" body={item.result} />
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}

function ActivityBody({
  label,
  body,
}: {
  readonly label: string;
  readonly body?: ThinkingActivityBody | null;
}) {
  if (body === undefined || body === null) return null;
  return (
    <details className="thinking__activitybody">
      <summary>{label}</summary>
      <pre>{body.text}</pre>
      {body.truncated ? <p>The recorded body was truncated.</p> : null}
    </details>
  );
}
