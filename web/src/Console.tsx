import { useMemo, useState } from 'react';
import { SessionTabs } from './sessions/SessionTabs';
import type { Session } from './sessions/types';
import { TerminalSurface } from './terminal/TerminalSurface';
import { createMockDriver } from './terminal/drivers/mock';
import { ProfileMenu } from './profile/ProfileMenu';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
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

let counter = 0;
function newSession(): Session {
  counter += 1;
  return {
    id: `s-${counter.toString(16).padStart(4, '0')}`,
    kernel: 'codex',
    lifecycle: 'ready',
    createdAt: Date.now(),
  };
}

export function Console({ user, onSignOut }: ConsoleProps) {
  /*
   * Read the landing-page prompt exactly once, on first render of the
   * authenticated surface. consumePendingTurn clears it as it reads, so a
   * refresh cannot resubmit the visitor's text a second time.
   */
  const [openingTurn] = useState<string | null>(() => consumePendingTurn());

  // Created in an initialiser rather than an effect: the first session is
  // initial state, not a reaction to anything, and an effect would render an
  // empty console for one frame before replacing it.
  const [firstSession] = useState<Session>(() => newSession());
  const [sessions, setSessions] = useState<readonly Session[]>([firstSession]);
  const [activeId, setActiveId] = useState<string | null>(firstSession.id);

  const active = sessions.find((s) => s.id === activeId) ?? null;

  /*
   * Keyed on the session so switching tabs builds a new driver, and memoised so
   * an unrelated re-render does not tear down and rebuild the terminal.
   */
  const driver = useMemo(() => {
    if (active === null) {
      return null;
    }
    return createMockDriver({
      sessionId: active.id,
      kernel: active.kernel,
      // Only the very first session inherits the landing prompt.
      openingTurn: active.id === sessions[0]?.id ? (openingTurn ?? undefined) : undefined,
    });
  }, [active, sessions, openingTurn]);

  function createSession() {
    const session = newSession();
    setSessions((prev) => [...prev, session]);
    setActiveId(session.id);
  }

  return (
    <div className="console">
      <header className="console__bar">
        <SessionTabs
          sessions={sessions}
          activeId={activeId}
          onSelect={setActiveId}
          onCreate={createSession}
        />
        <ProfileMenu user={user} onSignOut={onSignOut} />
      </header>

      <main className="console__body">
        {driver === null || active === null ? (
          <p className="console__empty">No session open. Press + to start one.</p>
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
