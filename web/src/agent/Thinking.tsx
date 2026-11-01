import { useEffect, useId, useRef, useState } from 'react';
import { CharacterAvatar } from '../character/CharacterAvatar';
import type { CharacterPose } from '../character/poses';
import './Thinking.css';
import { TurnUsage } from './TurnUsage';
import type { VisibleText } from './tokenEstimate';

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
  readonly text?: string;
  readonly activities?: readonly ThinkingActivity[];
  readonly startedAt?: string;
  readonly usage?: { readonly inputTokens: number; readonly outputTokens: number };
  readonly visibleText?: readonly VisibleText[];
  readonly visibleAnchor?: readonly VisibleText[];
  /** What the turn is doing right now (§28): thinking, working on a tool, or replying. */
  readonly pose?: CharacterPose;
}

export function summaryOf(text: string): string {
  return (
    text
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .at(-1) ?? ''
  );
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
  text = '',
  activities = [],
  startedAt,
  usage,
  visibleText,
  visibleAnchor,
  pose = 'thinking',
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

  const activity = activities.at(-1);
  const summary = activity === undefined ? summaryOf(text) : activitySummary(activity);
  return (
    <section className="thinking" aria-label="What the agent is working through">
      <button
        type="button"
        className="thinking__key"
        aria-expanded={open}
        aria-controls={open ? bodyId : undefined}
        onClick={() => setOpen((wasOpen) => !wasOpen)}
      >
        <CharacterAvatar pose={pose} size={32} className="thinking__character" />
        <span className="thinking__title">Thinking</span>
        {seconds === undefined ? null : <span className="thinking__elapsed">for {seconds}s</span>}
        <TurnUsage usage={usage} visibleText={visibleText} visibleAnchor={visibleAnchor} />
        <span className="thinking__dot" aria-hidden="true" />
        <span className="thinking__summary">{summary || 'Waiting for the next update'}</span>
        <span className="sr-only">, reasoning not kept after this turn</span>
      </button>
      {open ? (
        <div className="thinking__body" id={bodyId} ref={bodyRef}>
          {text === '' ? null : <div className="thinking__reasoning">{text}</div>}
          {text === '' && activities.length === 0 ? 'Waiting for the next update.' : null}
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
      ) : null}
    </section>
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
