import { useCallback, useEffect, useMemo, useState } from 'react';
import { SessionTabs } from './sessions/SessionTabs';
import type { Session, SessionLifecycle } from './sessions/types';
import { TerminalSurface } from './terminal/TerminalSurface';
import { createWebSocketDriver } from './terminal/drivers/websocket';
import { ProfileMenu } from './profile/ProfileMenu';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
import { ApiError, createSession, followSession, listSessions, terminalUrl } from './api/client';
import './Console.css';

/**
 * The authenticated surface. Same page, now driven live.
 *
 * Chrome is deliberately limited to the tab strip and the profile menu; the
 * product is one page and this must not grow navigation.
 */

interface ConsoleProps {
  readonly user: User;
  readonly onSignOut: () => void;
}

/**
 * Geometry the first frame is requested at.
 *
 * The surface reports its real size as soon as it measures, and the driver
 * holds any resize until the relay is ready, so this is only ever the opening
 * request rather than a size the terminal is stuck with.
 */
const INITIAL_COLS = 80;
const INITIAL_ROWS = 24;

export function Console({ user, onSignOut }: ConsoleProps) {
  /*
   * Read the landing-page prompt exactly once, on first render of the
   * authenticated surface. consumePendingTurn clears it as it reads, so a
   * refresh cannot resubmit the visitor's text a second time.
   */
  const [openingTurn] = useState<string | null>(() => consumePendingTurn());
  const [openingForId, setOpeningForId] = useState<string | null>(null);

  const [sessions, setSessions] = useState<readonly Session[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const active = sessions.find((s) => s.id === activeId) ?? null;

  const report = useCallback((cause: unknown) => {
    setNotice(cause instanceof ApiError ? cause.message : `something went wrong: ${String(cause)}`);
  }, []);

  /** Replace a session's lifecycle wherever it is listed. */
  const applyLifecycle = useCallback((id: string, lifecycle: SessionLifecycle) => {
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, lifecycle } : s)));
  }, []);

  /*
   * Sessions outlive the browser tab, so opening the console restores what is
   * already running. Nothing is created here: starting work is the owner's
   * decision, and a console that provisions a sandbox merely because someone
   * signed in spends real resources on an intention nobody expressed.
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const existing = await listSessions();
        if (cancelled) {
          return;
        }
        setSessions(existing);
        const live = existing.find((s) => s.lifecycle !== 'terminated');
        setActiveId(live?.id ?? null);
      } catch (cause) {
        if (!cancelled) {
          report(cause);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // Once, on open: re-running would fight the user's own tab selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const start = useCallback(async () => {
    setNotice(null);
    try {
      // Returns as soon as the session exists, before its runtime does, so the
      // tab appears immediately and reports its own progress.
      const session = await createSession('codex', INITIAL_COLS, INITIAL_ROWS);
      setSessions((prev) => [session, ...prev]);
      setActiveId(session.id);
      setOpeningForId((prev) => prev ?? session.id);
    } catch (cause) {
      report(cause);
    }
  }, [report]);

  /*
   * Follow whichever session is still starting. The stream is the only thing
   * that reports a runtime failing, since nobody is waiting on a request for
   * it any more.
   */
  useEffect(() => {
    if (active === null || active.lifecycle !== 'provisioning') {
      return;
    }
    const id = active.id;
    const stop = followSession(
      id,
      (lifecycle) => applyLifecycle(id, lifecycle),
      (message) => setNotice(message),
    );
    return stop;
  }, [active, applyLifecycle]);

  /*
   * Built only once a runtime exists. Attaching earlier would open a socket
   * against a session with nothing behind it yet.
   */
  // Whether this session has a runtime to attach to. Hoisted out of the memo
  // so the dependency is a plain value the linter can reason about.
  const attachable =
    active !== null && active.lifecycle !== 'provisioning' && active.lifecycle !== 'terminated';

  const driver = useMemo(() => {
    if (active === null || !attachable) {
      return null;
    }
    const turn = openingTurn !== null && active.id === openingForId ? openingTurn : undefined;
    return createWebSocketDriver({ url: terminalUrl(active.id), openingTurn: turn });
    // Rebuilt per session, not per render: an unrelated update must not tear
    // down a live terminal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id, attachable]);

  return (
    <div className="console">
      <header className="console__bar">
        <SessionTabs
          sessions={sessions}
          activeId={activeId}
          onSelect={setActiveId}
          onCreate={() => void start()}
        />
        <ProfileMenu user={user} onSignOut={onSignOut} />
      </header>

      <main className="console__body">
        {notice !== null ? (
          <p className="console__empty" role="alert">
            {notice}
          </p>
        ) : loading ? (
          <p className="console__empty">Loading your sessions…</p>
        ) : active === null ? (
          <p className="console__empty">No session open. Press + to start one.</p>
        ) : active.lifecycle === 'provisioning' ? (
          <p className="console__empty" aria-live="polite">
            Starting a sandbox for this session. This can take a minute the first time.
          </p>
        ) : active.lifecycle === 'terminated' ? (
          <p className="console__empty">This session has ended. Press + to start another.</p>
        ) : driver === null ? (
          <p className="console__empty">Attaching…</p>
        ) : (
          <TerminalSurface
            key={active.id}
            driver={driver}
            label={`Session ${active.id} on ${active.kernel}`}
          />
        )}
      </main>
    </div>
  );
}
