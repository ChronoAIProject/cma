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
import { useSessionDrivers } from './sessions/drivers';
import { SessionCanvas } from './canvas/SessionCanvas';
import { ViewModeToggle } from './view/ViewModeToggle';
import { loadViewMode, saveViewMode } from './view/mode';
import type { ViewMode } from './view/mode';
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
} from './api/client';
import { abilitiesFor, listable } from './sessions/availability';
import './Console.css';

/**
 * The authenticated surface. Same page, now driven live.
 *
 * Chrome is deliberately limited to the session rail, the view-mode toggle and
 * the profile menu; the product is one page and this must not grow navigation.
 *
 * The two view modes are two arrangements of one state. `activeId` means the
 * session the keyboard is talking to in both of them — in tabs mode that is the
 * one visible terminal, on the canvas it is the focused node — so switching
 * arrangement never asks anyone where they were.
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
  const [mode, setMode] = useState<ViewMode>(() => loadViewMode());
  /*
   * Most recently selected first. The canvas spends its attachment budget from
   * this, so it is kept here rather than there: every route that changes the
   * selection goes through this component, including the rail, and a history
   * the canvas kept for itself would forget everything on a switch to tabs and
   * back.
   */
  const [focusHistory, setFocusHistory] = useState<readonly string[]>([]);

  const active = sessions.find((s) => s.id === activeId) ?? null;

  /** Select a session, in whichever arrangement is showing. */
  const select = useCallback((id: string) => {
    setActiveId(id);
    setFocusHistory((prev) =>
      prev[0] === id ? prev : [id, ...prev.filter((seen) => seen !== id)],
    );
  }, []);

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
        if (live !== undefined) {
          select(live.id);
        }
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
        select(session.id);
        setOpeningForId((prev) => prev ?? session.id);
      } catch (cause) {
        report(cause);
      }
    },
    [report, select],
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
        setSessions((prev) => prev.filter((s) => s.id !== id));
        // Out of the history as well as out of the list: a deleted id left
        // there is a slot of the canvas's attachment budget spent remembering
        // a session nobody can open.
        setFocusHistory((prev) => prev.filter((seen) => seen !== id));
        if (activeId === id) {
          /*
           * The successor is *selected*, not merely made active. `activeId` is
           * not the whole selection — the canvas spends its attachment budget
           * from `focusHistory` — so a session promoted straight into
           * `activeId` would be the focused node while sitting at the back of
           * that list, and with a full history it is evicted at once. That is
           * the one node §15 promises is never detached.
           */
          const successor = sessions.find((s) => s.id !== id);
          if (successor === undefined) {
            setActiveId(null);
          } else {
            select(successor.id);
          }
        }
      } catch (cause) {
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [activeId, report, select, sessions],
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
   * One driver per session, whichever arrangement is showing. Held for every
   * session rather than only the open one because the canvas renders them all —
   * and building a driver costs nothing until a surface attaches it, so the
   * sessions nobody is looking at open no connections.
   *
   * Built from every session rather than only the listed ones. A session that
   * ended on its own leaves the list, but whoever had it open is still owed the
   * last thing it said.
   */
  const opening = useMemo(
    () =>
      openingTurn === null || openingForId === null
        ? null
        : { text: openingTurn, sessionId: openingForId },
    [openingTurn, openingForId],
  );
  const { nodes, reconnect } = useSessionDrivers(sessions, opening);
  const driver = nodes.find((node) => node.session.id === activeId)?.driver ?? null;

  // The canvas shows exactly what the rail lists, so one arrangement never
  // holds a session the other has let go of.
  const listed = useMemo(() => listable(sessions), [sessions]);
  const canvasNodes = useMemo(
    () => nodes.filter((node) => abilitiesFor(node.session).listed),
    [nodes],
  );

  const changeMode = useCallback((next: ViewMode) => {
    setMode(next);
    saveViewMode(next);
  }, []);

  return (
    <div className="console">
      <TopBar>
        {/*
         * A display preference rather than navigation: it changes how the
         * sessions someone already has are arranged, and reaches nothing else.
         * It sits at the trailing edge with the account control because it
         * belongs to the whole console rather than to any one column.
         */}
        <ViewModeToggle mode={mode} onChange={changeMode} />
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
          sessions={listed}
          activeId={activeId}
          order={order}
          onSelect={select}
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
          ) : mode === 'canvas' && canvasNodes.length > 0 ? (
            <SessionCanvas
              /*
               * Remounted when the palette changes, for the same reason the
               * single terminal is: xterm reads its theme once, at
               * construction. Doing it on an explicit choice rather than
               * continuously is what keeps a live session from being torn down
               * underneath somebody.
               */
              key={`canvas:${String(paletteEpoch)}`}
              nodes={canvasNodes}
              focusedId={activeId}
              focusHistory={focusHistory}
              onFocus={select}
            />
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
              /*
               * §14: whether a terminal takes typing is the session's state
               * saying so, never the fact that it is the one on screen. The
               * driver for a stopped session drops input too — this is the
               * layer that stops xterm raising the event at all.
               */
              interactive={abilitiesFor(active).interactive}
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
                  /*
                   * A new driver for this session and no other. The surface
                   * holding the old one releases its transport and the new one
                   * dials again, which asks the control plane to take the
                   * sandbox back under management.
                   *
                   * Scoped deliberately: this used to bump `paletteEpoch`,
                   * which is also the canvas's key, so one session's Reconnect
                   * rebuilt every node on the canvas — every scrollback
                   * cleared, every socket redialled.
                   */
                  setNotice(null);
                  reconnect(active.id);
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
