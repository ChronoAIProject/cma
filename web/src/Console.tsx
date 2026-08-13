import { useCallback, useEffect, useMemo, useState } from 'react';
import { SessionTabs } from './sessions/SessionTabs';
import type { Session } from './sessions/types';
import { TerminalSurface } from './terminal/TerminalSurface';
import { createWebSocketDriver } from './terminal/drivers/websocket';
import { ProfileMenu } from './profile/ProfileMenu';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
import { ApiError, createSession, listSessions, terminalUrl } from './api/client';
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
  /*
   * Which session the landing prompt belongs to. Bound once, when the first
   * session of this page load is established, so opening a second tab cannot
   * replay someone's opening line into it.
   */
  const [openingForId, setOpeningForId] = useState<string | null>(null);

  const [sessions, setSessions] = useState<readonly Session[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);

  const active = sessions.find((s) => s.id === activeId) ?? null;

  const report = useCallback((cause: unknown) => {
    const message =
      cause instanceof ApiError ? cause.message : `something went wrong: ${String(cause)}`;
    setNotice(message);
  }, []);

  const start = useCallback(async () => {
    setBusy(true);
    setNotice(null);
    try {
      const session = await createSession('codex', INITIAL_COLS, INITIAL_ROWS);
      setSessions((prev) => [session, ...prev]);
      setActiveId(session.id);
      setOpeningForId((prev) => prev ?? session.id);
    } catch (cause) {
      report(cause);
    } finally {
      setBusy(false);
    }
  }, [report]);

  /*
   * Sessions outlive the browser tab, so the console opens onto whatever is
   * already running rather than starting something new. Only an owner with no
   * sessions at all gets one created for them, which is what makes the first
   * visit land straight in a terminal.
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const existing = await listSessions();
        if (cancelled) {
          return;
        }
        const live = existing.filter((s) => s.lifecycle !== 'terminated');
        if (live.length > 0) {
          const resumed = live[0]?.id ?? null;
          setSessions(existing);
          setActiveId(resumed);
          setOpeningForId((prev) => prev ?? resumed);
          setBusy(false);
          return;
        }
        await start();
      } catch (cause) {
        if (!cancelled) {
          report(cause);
          setBusy(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // Deliberately once: this restores what already exists, and re-running it
    // on any state change would fight the user's own tab selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /*
   * Keyed on the session so switching tabs builds a new driver, and memoised so
   * an unrelated re-render does not tear down and rebuild the terminal.
   */
  const driver = useMemo(() => {
    if (active === null) {
      return null;
    }
    // The landing prompt goes to the session it was bound to and no other.
    const turn = openingTurn !== null && active.id === openingForId ? openingTurn : undefined;
    return createWebSocketDriver({ url: terminalUrl(active.id), openingTurn: turn });
    // Rebuilt per session, not per render: an unrelated update must not tear
    // down a live terminal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id]);

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
        ) : driver === null || active === null ? (
          <p className="console__empty">
            {busy ? 'Opening a session…' : 'No session open. Press + to start one.'}
          </p>
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
