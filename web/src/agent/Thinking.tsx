import { useEffect, useId, useRef, useState } from 'react';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
import { elapsedTime } from './workspaceTime';
import { ThinkingProgress, type recordedTurnActivity } from './ThinkingProgress';
const markHtml = { __html: markMarkup };
import './Thinking.css';

export interface ThinkingActivityBody {
  readonly text: string;
  readonly truncated: boolean;
}

/** A currently pending tool call, removed from this disclosure when it settles. */
export interface ThinkingActivity {
  readonly id: string;
  readonly kind: string;
  readonly target: string;
  readonly status: string;
  readonly durationMs?: number | null;
  readonly arguments?: ThinkingActivityBody | null;
  readonly result?: ThinkingActivityBody | null;
  readonly detail?: ThinkingActivityBody;
}

interface ThinkingProps {
  readonly branded?: boolean;
  readonly waitingForResponse?: boolean;
  readonly progressKey?: string;
  readonly text?: string;
  readonly activities?: readonly ThinkingActivity[];
  readonly startedAt?: string;
  readonly recordedActivity?: ReturnType<typeof recordedTurnActivity>;
}

export function activityDuration(activity: ThinkingActivity): string {
  const duration = activity.durationMs;
  if (duration === undefined || duration === null) {
    return ['started', 'pending', 'running', 'in_progress'].includes(activity.status.toLowerCase())
      ? 'ongoing'
      : 'not reported';
  }
  if (duration < 1000) return `${String(duration)}ms`;
  const seconds = duration / 1000;
  return `${Number.isInteger(seconds) ? String(seconds) : seconds.toFixed(1)}s`;
}

export function activitySummary(activity: ThinkingActivity): string {
  return `${activity.kind} - ${activity.target} - ${activity.status} - ${activityDuration(activity)}`;
}

/** Active-turn liveness, with a clock anchored to the public turn record rather than mount. */
export function Thinking({
  branded = false,
  text = '',
  activities = [],
  startedAt,
  progressKey,
  waitingForResponse = false,
  recordedActivity,
}: ThinkingProps) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  const bodyRef = useRef<HTMLDivElement>(null);
  const bodyId = useId();
  const start = Date.parse(startedAt ?? '');
  const seconds = Number.isFinite(start)
    ? Math.max(0, Math.floor((now - start) / 1000))
    : undefined;

  useEffect(() => {
    if (!Number.isFinite(start)) return;
    const update = () => setNow(Date.now());
    const timer = window.setInterval(update, 1000);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', update);
    };
  }, [start]);

  useEffect(() => {
    const body = bodyRef.current;
    if (body !== null && open) body.scrollTop = body.scrollHeight;
  }, [activities, text, open]);

  return (
    <section className="thinking" aria-label="What the agent is working through">
      <div className="thinking__key">
        {branded ? null : (
          <>
            <span
              className="thinking__logo"
              aria-hidden="true"
              dangerouslySetInnerHTML={markHtml}
            />
            <span className="thinking__title" role="status">
              {waitingForResponse ? 'Waiting for a response' : 'Thinking'}
            </span>
          </>
        )}
        {seconds === undefined ? null : (
          <span className="thinking__elapsed">{elapsedTime(seconds * 1000)}</span>
        )}
        {branded && seconds === undefined ? null : <span aria-hidden="true">·</span>}
        <button
          type="button"
          className="thinking__activity-toggle"
          aria-expanded={open}
          aria-controls={open ? bodyId : undefined}
          onClick={() => setOpen((value) => !value)}
        >
          View activity
        </button>
      </div>
      <p className="message__delivery">
        Still waiting for this turn to finish.{' '}
        {Number.isFinite(start) ? (
          <>
            Turn started <time dateTime={startedAt}>{new Date(start).toLocaleString()}</time>.
          </>
        ) : (
          'Its start time is unavailable.'
        )}
      </p>
      <ThinkingProgress key={progressKey ?? JSON.stringify([text, activities])} />
      {open ? (
        <div id={bodyId}>
          {recordedActivity === undefined ? null : (
            <dl className="thinking__facts">
              <dt>Current phase</dt>
              <dd>{recordedActivity.phase}</dd>
              <dt>Phase started</dt>
              <dd>
                <ActivityTime value={recordedActivity.since} />
              </dd>
              <dt>Last recorded progress</dt>
              <dd>
                <ActivityTime value={recordedActivity.lastAt} />
              </dd>
            </dl>
          )}
          {text === '' && activities.length === 0 ? null : (
            <div className="thinking__body" ref={bodyRef}>
              {text === '' ? null : <div className="thinking__reasoning">{text}</div>}
              {activities.length === 0 ? null : (
                <ul className="thinking__activities">
                  {activities.map((item) => (
                    <li className="thinking__activity" key={item.id}>
                      <p className="thinking__activityline">{activitySummary(item)}</p>
                      <ActivityBody label="Arguments" body={item.arguments} />
                      <ActivityBody label="Result" body={item.result} />
                      <ActivityBody label="Details" body={item.detail} />
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      ) : null}
    </section>
  );
}

function ActivityTime({ value }: { readonly value?: string }) {
  const timestamp = Date.parse(value ?? '');
  return Number.isFinite(timestamp) ? (
    <time dateTime={value}>{new Date(timestamp).toLocaleString()}</time>
  ) : (
    'Unavailable'
  );
}

export function ActivityBody({
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
