import { useCallback, useEffect, useMemo, useState } from 'react';
import { SessionRail } from './sessions/SessionRail';
import { SessionDetails } from './sessions/SessionDetails';
import { TopBar } from './shell/TopBar';
import type { SortOrder } from './sessions/sessionTime';
import type { Kernel, Session, SessionLifecycle } from './sessions/types';
import { NewSessionDialog } from './sessions/NewSessionDialog';
import { StartupProgress } from './sessions/StartupProgress';
import type { Stage } from './sessions/progress';
import { SessionActions } from './sessions/SessionActions';
import type { AgentOptions } from './sessions/options';
import { TerminalSurface } from './terminal/TerminalSurface';
import { createWebSocketDriver } from './terminal/drivers/websocket';
import { createTranscriptDriver } from './terminal/drivers/transcript';
import { ProfileMenu } from './profile/ProfileMenu';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
import {
  ApiError,
  createSession,
  deleteSession,
  followSession,
  listSessions,
  reviveSession,
  stopSession,
  terminalUrl,
} from './api/client';
import { abilitiesFor, listable } from './sessions/availability';
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
  /*
   * Bumped when the terminal palette changes. xterm reads its theme when the
   * terminal is constructed, so remounting is what applies a new one — and
   * doing it on an explicit change rather than continuously means a live
   * session is never torn down underneath someone.
   */
  const [paletteEpoch, setPaletteEpoch] = useState(0);
  // Creating a session asks first. These choices are fixed for its life, so
  // there is no changing them afterwards without discarding the conversation.
  const [choosing, setChoosing] = useState(false);
  // Why a session ended, when it ended badly. Kept per session so switching
  // tabs does not carry one session's failure onto another.
  const [endings, setEndings] = useState<Record<string, string>>({});
  const [acting, setActing] = useState(false);
  // The last startup step each session reported. Kept per session so switching
  // tabs shows that session's progress rather than another's.
  const [stages, setStages] = useState<Record<string, Stage>>({});
  const [order, setOrder] = useState<SortOrder>('newest');
  // Open by default: someone arriving wants to know what they are looking at,
  // and the panel is the only place that says.
  const [detailsOpen, setDetailsOpen] = useState(true);
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

  const start = useCallback(
    async (kernel: Kernel, options: AgentOptions) => {
      setNotice(null);
      try {
        // Returns as soon as the session exists, before its runtime does, so the
        // tab appears immediately and reports its own progress.
        const session = await createSession(kernel, INITIAL_COLS, INITIAL_ROWS, options);
        setSessions((prev) => [session, ...prev]);
        setActiveId(session.id);
        setOpeningForId((prev) => prev ?? session.id);
      } catch (cause) {
        report(cause);
      }
    },
    [report],
  );

  /** End the open session, leaving it readable. */
  const endSession = useCallback(
    async (id: string) => {
      setActing(true);
      try {
        // Stop rather than terminate: ending a session releases what it is
        // costing without throwing away what it did, and the owner can pick it
        // up again. Terminating is what delete does on the way out.
        await stopSession(id);
        applyLifecycle(id, 'idle');
      } catch (cause) {
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [applyLifecycle, report],
  );

  /** Give a stopped session a sandbox again, and follow it back up. */
  const reviveSessionById = useCallback(
    async (id: string) => {
      setActing(true);
      try {
        const revived = await reviveSession(id);
        // Back to provisioning, which is what the effect below follows: the
        // progress of a revive is the progress of a start, because it is one.
        setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, ...revived } : s)));
      } catch (cause) {
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [report],
  );

  /** Remove the session and its history. */
  const removeSession = useCallback(
    async (id: string) => {
      setActing(true);
      try {
        await deleteSession(id);
        setSessions((prev) => {
          const remaining = prev.filter((s) => s.id !== id);
          setActiveId((current) => (current === id ? (remaining[0]?.id ?? null) : current));
          return remaining;
        });
      } catch (cause) {
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [report],
  );

  /*
   * Follow whichever session is still starting. The stream is the only thing
   * that reports a runtime failing, since nobody is waiting on a request for
   * it any more.
   */
  useEffect(() => {
    if (
      active === null ||
      (active.lifecycle !== 'provisioning' && active.lifecycle !== 'rotating')
    ) {
      return;
    }
    const id = active.id;
    const stop = followSession(
      id,
      (lifecycle, detail) => {
        applyLifecycle(id, lifecycle);
        if (detail !== undefined && detail !== '') {
          setEndings((prev) => ({ ...prev, [id]: detail }));
        }
      },
      (message) => {
        setNotice(message);
      },
      (stage) => {
        setStages((prev) => ({ ...prev, [id]: stage }));
      },
    );
    return stop;
    // Keyed on the session rather than the object: `active` is rebuilt on
    // every render, and depending on it would tear the stream down and reopen
    // it continuously.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id, active?.lifecycle, applyLifecycle]);

  /*
   * Built only once a runtime exists. Attaching earlier would open a socket
   * against a session with nothing behind it yet.
   */
  // Whether this session has a runtime to attach to. Hoisted out of the memo
  // so the dependency is a plain value the linter can reason about.
  // One rule for whether a session can be typed into, shared with the buttons
  // that act on it. A terminal that accepts keystrokes the control plane will
  // refuse is worse than one that plainly says it is not accepting them.
  const attachable = active !== null && abilitiesFor(active).interactive;

  const driver = useMemo(() => {
    if (active === null) {
      return null;
    }
    if (!attachable) {
      // No sandbox to attach to, but a history to read. Someone who stopped a
      // session did not ask to lose sight of it, and someone whose session
      // failed usually wants to read the last thing it said.
      const can = abilitiesFor(active);
      return can.canRevive || active.lifecycle === 'terminated'
        ? createTranscriptDriver(active.id, can.reason ?? '')
        : null;
    }
    const turn = openingTurn !== null && active.id === openingForId ? openingTurn : undefined;
    return createWebSocketDriver({ url: terminalUrl(active.id), openingTurn: turn });
    // Rebuilt per session, not per render: an unrelated update must not tear
    // down a live terminal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id, active?.lifecycle, attachable]);

  return (
    <div className="console">
      <TopBar>
        <ProfileMenu
          user={user}
          onSignOut={onSignOut}
          onTerminalThemeChange={() => {
            setPaletteEpoch((n) => n + 1);
          }}
        />
      </TopBar>

      <div className="console__body">
        <SessionRail
          sessions={listable(sessions)}
          activeId={activeId}
          order={order}
          onSelect={setActiveId}
          onCreate={() => {
            setChoosing(true);
          }}
          onReorder={setOrder}
        />

        <main className="console__surface">
          {notice !== null ? (
            <p className="console__empty" role="alert">
              {notice}
            </p>
          ) : loading ? (
            <p className="console__empty">Loading your sessions…</p>
          ) : active === null ? (
            <p className="console__empty">No session open. Start one from the left.</p>
          ) : active.lifecycle === 'provisioning' ? (
            <StartupProgress
              message="Starting a sandbox for this session. This can take a minute the first time."
              stage={stages[active.id] ?? null}
            />
          ) : active.lifecycle === 'rotating' ? (
            <StartupProgress
              message="Moving this session to an updated runtime. It will come back where it left off."
              stage={stages[active.id] ?? null}
            />
          ) : driver === null ? (
            <p className="console__empty">Attaching…</p>
          ) : (
            <TerminalSurface
              key={`${active.id}:${String(paletteEpoch)}`}
              driver={driver}
              label={`Session ${active.id} on ${active.kernel}`}
            />
          )}
        </main>

        {active !== null ? (
          <SessionDetails
            session={active}
            open={detailsOpen}
            ending={endings[active.id]}
            onToggle={() => {
              setDetailsOpen((v) => !v);
            }}
            actions={
              <SessionActions
                session={active}
                busy={acting}
                onReconnect={() => {
                  // Rebuilding the driver reattaches, which asks the control
                  // plane to take the sandbox back under management.
                  setNotice(null);
                  setPaletteEpoch((n) => n + 1);
                }}
                onRevive={() => void reviveSessionById(active.id)}
                onStop={() => void endSession(active.id)}
                onDelete={() => void removeSession(active.id)}
              />
            }
          />
        ) : null}
      </div>

      <NewSessionDialog
        open={choosing}
        onCancel={() => {
          setChoosing(false);
        }}
        onStart={(kernel, options) => {
          setChoosing(false);
          void start(kernel as Kernel, options);
        }}
      />
    </div>
  );
}
