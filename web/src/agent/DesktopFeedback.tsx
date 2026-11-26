import { useEffect, useState } from 'react';
import { Presence } from '../ui/Presence';
import { Skeleton } from '../ui/Skeleton';
import { StateDot } from '../ui/StateDot';
import type { DesktopPhase } from './useDesktopConnection';

export type DesktopControl = 'view' | 'user' | 'agent';

/** Changes replace the previous notice and get a fresh five-second lifetime. */
export function useDesktopNotice(phase: DesktopPhase, control: DesktopControl) {
  const mode = phase === 'ready' ? control : phase;
  const [previous, setPrevious] = useState(mode);
  const [notice, setNotice] = useState<{ text: string } | null>(null);
  if (mode !== previous) {
    setPrevious(mode);
    const text =
      mode === 'user'
        ? 'You have control. Click and type on the desktop.'
        : mode === 'agent'
          ? 'The Agent has control. Watch its actions here.'
          : mode === 'view' && previous === 'agent'
            ? 'The Agent is paused. The desktop is idle.'
            : mode === 'view' && previous === 'user'
              ? 'Control released. You are watching the desktop.'
              : null;
    setNotice(text ? { text } : null);
  }
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), 5_000);
    return () => window.clearTimeout(timer);
  }, [notice]);
  return notice;
}

export function DesktopFeedback({
  phase,
  error,
  notice,
}: {
  readonly phase: DesktopPhase;
  readonly error: string;
  readonly notice: { text: string } | null;
}) {
  if (phase === 'error')
    return (
      <p className="agent-desktop__loading agent-desktop__loading--error" aria-hidden="true">
        {error}
      </p>
    );
  return (
    <>
      {phase !== 'ready' && (
        <div className="agent-desktop__loading" aria-hidden="true">
          <div className="agent-desktop__placeholder">
            <div className="agent-desktop__placeholder-bar">
              <Skeleton width="28%" height={8} />
              <Skeleton width="12%" height={8} />
            </div>
            <div className="agent-desktop__placeholder-window">
              <Skeleton width="40%" />
              <Skeleton width="80%" />
              <Skeleton width="60%" />
            </div>
          </div>
          <div className="agent-desktop__loading-label">
            <StateDot state="ongoing" />
            <span>
              {phase === 'waiting'
                ? 'Waiting for your machine'
                : phase === 'connecting'
                  ? 'Connecting to your machine'
                  : phase === 'recovering'
                    ? 'Reconnecting…'
                    : phase === 'loading'
                      ? 'Loading your desktop'
                      : error}
            </span>
          </div>
        </div>
      )}
      <Presence>
        {notice && (
          // The persistent header announces the same state once. This visual
          // feedback never captures focus, pointer input or keyboard events.
          <div className="agent-desktop__notice" aria-hidden="true">
            {notice.text}
          </div>
        )}
      </Presence>
    </>
  );
}
