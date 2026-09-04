import { useEffect, useRef, useState } from 'react';
import './Thinking.css';
import { IconThinkOutline14 } from '../ui/icons';

/**
 * The agent's exposed working notes — `Web_Style.md` §18.8.
 *
 * Reasoning, intermediate public messages, and tool activity arrive here while
 * the turn runs. Their complete public form is retained at the turn boundary,
 * so the same open container can be reconstructed after navigation or reload.
 * The disclosure remains height-bounded and resizable because the work can be
 * much longer than the final answer.
 *
 * No colour, ever. §2 reserves the semantic inks for session and system
 * state, and working notes are neither — they say nothing about whether the
 * session itself is healthy.
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
  /** Live or retained reasoning summary explicitly exposed by the agent. */
  readonly text?: string;
  /** Public intermediate messages superseded by the turn's final answer. */
  readonly messages?: readonly string[];
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

export function Thinking({
  text = '',
  messages = [],
  activities = [],
  live = true,
}: ThinkingProps) {
  const [open, setOpen] = useState(true);
  const bodyRef = useRef<HTMLDivElement>(null);
  /** How long this live working-notes stream has been arriving. */
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
    // Keep the newest live work visible. A settled container remains manually
    // scrollable and this effect no longer runs once its contents stop changing.
    const body = bodyRef.current;
    if (body !== null && open) {
      body.scrollTop = body.scrollHeight;
    }
  }, [activities, messages, text, open]);

  const recordedDuration = activities.reduce<number | undefined>((longest, activity) => {
    const duration = activity.durationMs;
    if (duration === undefined || duration === null) return longest;
    return longest === undefined ? duration : Math.max(longest, duration);
  }, undefined);
  const title =
    live || recordedDuration !== undefined
      ? `Thought for ${String(live ? seconds : Math.ceil((recordedDuration ?? 0) / 1000))}s`
      : 'Working notes';
  const activity = activities.at(-1);
  const summary =
    summaryOf(messages.at(-1) ?? '') ||
    summaryOf(text) ||
    (activity === undefined ? '' : activitySummary(activity));

  return (
    <section
      className={`thinking${live ? '' : ' thinking--settled'}`}
      aria-label="Agent working notes"
    >
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
        {open ? null : (
          <>
            <span className="thinking__dot" aria-hidden="true" />
            {/* The latest line identifies useful work while the body is shut. */}
            <span className="thinking__summary">{summary}</span>
          </>
        )}
        <span className="sr-only">, retained in this conversation</span>
      </button>

      {/*
       * Not a live region. It changes many times a second, and announcing
       * each change would make the surface unusable with a screen reader; the
       * message that follows is what is worth hearing, and it is announced.
       */}
      {open ? (
        <div className="thinking__body" ref={bodyRef}>
          {text === '' ? null : <div className="thinking__reasoning">{text}</div>}
          {messages.length === 0 ? null : (
            <div className="thinking__messages">
              {messages.map((message, index) => (
                <div className="thinking__message" key={index}>
                  {message}
                </div>
              ))}
            </div>
          )}
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
    <details className="thinking__activitybody" open>
      <summary>{label}</summary>
      <pre>{body.text}</pre>
      {body.truncated ? <p>The recorded body was truncated.</p> : null}
    </details>
  );
}
