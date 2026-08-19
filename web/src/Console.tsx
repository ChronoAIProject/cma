import { useCallback, useEffect, useMemo, useState } from 'react';
import { SessionRail } from './sessions/SessionRail';
import { SessionDrawer } from './sessions/SessionDrawer';
import { AppFrame } from './shell/AppFrame';
import { Sidebar } from './shell/Sidebar';
import type { SortOrder } from './sessions/sessionTime';
import type { KernelOffer, Session, SessionLifecycle } from './sessions/types';
import { NewSessionDialog } from './sessions/NewSessionDialog';
import { StartupProgress } from './sessions/StartupProgress';
import type { Stage } from './sessions/progress';
import { SessionActions } from './sessions/SessionActions';
import { SessionHeader } from './sessions/SessionHeader';
import { TerminalSurface } from './terminal/TerminalSurface';
import { ConversationSurface } from './chat/ConversationSurface';
import { useSessionDrivers } from './sessions/drivers';
import { SessionCanvas } from './canvas/SessionCanvas';
import { selectLiveWindow, unattachedNote } from './canvas/layout';
import { loadArrangement, saveArrangement } from './sessions/arrangement';
import type { Arrangement } from './sessions/arrangement';
import { ProfileMenu } from './profile/ProfileMenu';
import { CommandBar } from './shell/CommandBar';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
import { ApiError, followSession, listKernels, listSessions } from './api/client';
import { abilitiesFor, listable } from './sessions/availability';
import { useSessionActions, type FailureNotice } from './sessions/useSessionActions';
import { EnvironmentProfiles } from './environment/EnvironmentProfiles';
import { createRestProfileStore } from './environment/rest';
import type { EnvironmentProfile, EnvironmentProfileStore } from './environment/types';
import { OperatorPage } from './operator/OperatorPage';
import { canOperate } from './operator/access';
import { createRestOperatorStore } from './operator/rest';
import type { OperatorStore } from './operator/types';
import './Console.css';

/**
 * The authenticated surface. Same page, now driven live.
 *
 * Two views inside one page frame, per docs/Web_Style.md §1: sessions and
 * operations. There is no router and no URL — the app bar's switch chooses
 * between them and nothing is pushed onto history, which is what keeps the
 * frame from becoming navigation.
 *
 * **The view does not persist; the arrangement does.** A view is where
 * somebody is right now, and reloading into a surface nobody asked for is how
 * a page frame starts behaving like a route. An arrangement is a setting about
 * how a list is drawn, which is exactly the kind of thing a browser should
 * remember.
 *
 * The two arrangements are two arrangements of one state. `activeId` means the
 * session the keyboard is talking to in both of them — in the list that is
 * the one visible terminal, on the canvas it is the focused node — so switching
 * arrangement never asks anyone where they were.
 */

interface ConsoleProps {
  readonly user: User;
  readonly onSignOut: () => void;
}

/** The two views §1 admits. Not a route, and never a third. */
type ConsoleView = 'sessions' | 'operations';

/**
 * Geometry the first frame is requested at.
 *
 * The surface reports its real size as soon as it measures, and the driver
 * holds any resize until the relay is ready, so this is only ever the opening
 * request rather than a size the terminal is stuck with.
 */

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
  const [notice, setNotice] = useState<FailureNotice | null>(null);
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
  // Built once. A store rebuilt on every render would refetch on every render,
  // because everything consuming it loads in an effect keyed on its identity.
  const [profileStore] = useState<EnvironmentProfileStore>(() => createRestProfileStore());
  const [profiles, setProfiles] = useState<readonly EnvironmentProfile[]>([]);
  /*
   * What this deployment can run, and what each runtime can be. Asked once and
   * passed to every form that needs it: two askers are two answers that can
   * disagree while somebody is choosing between them.
   */
  const [kernels, setKernels] = useState<readonly KernelOffer[]>([]);
  const [managingProfiles, setManagingProfiles] = useState(false);
  // Built once, for the same reason the profile store is.
  const [operatorStore] = useState<OperatorStore>(() => createRestOperatorStore());
  /*
   * Which of §1's two views is showing. State rather than a route: the app
   * carries no router, and this is deliberately not persisted — see above.
   */
  const [view, setView] = useState<ConsoleView>('sessions');

  useEffect(() => {
    void profileStore
      .list()
      .then(setProfiles)
      .catch(() => {
        // A console that cannot list profiles is still a console. The manager
        // reports the failure when someone actually opens it.
      });
  }, [profileStore]);

  useEffect(() => {
    void listKernels()
      .then(setKernels)
      .catch(() => {
        // An empty list is "nothing is known", which the forms draw as an
        // offer to author a profile they cannot honour — rather than as a
        // deployment that runs nothing.
      });
  }, []);
  // The last startup step each session reported. Kept per session so switching
  // tabs shows that session's progress rather than another's.
  const [stages, setStages] = useState<Record<string, Stage>>({});
  const [order, setOrder] = useState<SortOrder>('newest');
  /*
   * The details drawer — §12: closed by default, opened from the header's
   * Details key. Not persisted: it is reference material opened on demand,
   * not a layout choice to carry across visits.
   */
  const [detailsOpen, setDetailsOpen] = useState(false);
  /*
   * Which session is parked on an answerable approval — §18.3: it says so in
   * the rail, the header and at the composer. Only the conversation surface
   * folds the log, so it reports the fact up and this is where it lands.
   */
  const [waitingId, setWaitingId] = useState<string | null>(null);
  /*
   * The command bar plate — §6. Opened from the bar's trigger and by ⌘K;
   * closing returns focus to whatever opened it via the plate machinery.
   */
  const [commandOpen, setCommandOpen] = useState(false);

  /*
   * §6: ⌘K opens and closes the command bar. On the document, because the
   * shortcut has to work wherever the caret is — that is the whole point of
   * having one.
   */
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setCommandOpen((open) => !open);
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
    };
  }, []);
  // Validated on read: web storage is untrusted input, and an unrecognised
  // value reaching the layout would be a rail nobody can draw (§7).
  const [railCollapsed, setRailCollapsed] = useState(false);
  /* Below 60rem the rail is a drawer over the layout rather than a column. */
  const [loading, setLoading] = useState(true);
  const [arrangement, setArrangement] = useState<Arrangement>(() => loadArrangement());
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

  /*
   * §12: the card carries what failed — ending with the API's own words
   * where it gave any — what was not changed, and the API's code in
   * identifier style. A client-side failure has no code to show.
   */
  const report = useCallback((cause: unknown, failed: string, unchanged: string) => {
    if (cause instanceof ApiError) {
      setNotice({
        failed: `${failed} ${cause.message}`,
        unchanged,
        code: cause.status > 0 ? `HTTP ${String(cause.status)}` : undefined,
      });
      return;
    }
    setNotice({ failed: `${failed} ${String(cause)}`, unchanged });
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
          report(cause, 'Loading your sessions failed.', 'Nothing was changed.');
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

  const { start, endSession, reviveSessionById, removeSession, changeSessionPlane } =
    useSessionActions({
      sessions,
      activeId,
      setSessions,
      setActiveId,
      setOpeningForId,
      setNotice,
      setActing,
      select,
      report,
      applyLifecycle,
      setFocusHistory,
    });

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
        setNotice({ failed: message });
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

  const changeArrangement = useCallback((next: Arrangement) => {
    setArrangement(next);
    saveArrangement(next);
  }, []);

  /*
   * Collapsing changes no session's state. It is a display preference about
   * this window — and about this window only: §9 makes widths transient, so
   * it is not written down. A visit on a laptop does not decide what a desk
   * looks like.
   */
  const toggleRail = useCallback(() => {
    setRailCollapsed((wasCollapsed: boolean) => !wasCollapsed);
  }, []);

  const arrangementShown: Arrangement = arrangement;

  /*
   * What the canvas's attachment budget cost, for the status band (§13).
   * Truncation is never silent, and the band is the one part of the page that
   * cannot be panned out of sight.
   */
  const note = useMemo(() => {
    if (view !== 'sessions' || arrangementShown !== 'canvas') {
      return undefined;
    }
    const connectable = canvasNodes.filter((node) => node.driver !== null).map((n) => n.session.id);
    return unattachedNote(connectable.length, selectLiveWindow(connectable, focusHistory).size);
  }, [view, arrangementShown, canvasNodes, focusHistory]);

  /*
   * What the status band says this page is showing (§6).
   *
   * Counted from what is actually drawn rather than from everything the
   * control plane returned: an ended session leaves the list, and a band
   * naming a session nobody can see would send its owner looking for it.
   */
  /*
   * Counted from what is actually drawn rather than from everything the
   * control plane returned, for the same reason as `showing` below.
   */
  const liveCount = useMemo(
    () => listed.filter((session) => abilitiesFor(session).interactive).length,
    [listed],
  );

  const showing = useMemo(() => {
    if (view === 'operations') {
      return 'Operations';
    }
    if (loading) {
      return 'Loading sessions';
    }
    if (listed.length === 0) {
      return 'No sessions';
    }
    const count = `${String(listed.length)} session${listed.length === 1 ? '' : 's'}`;
    return `${count} · ${String(liveCount)} live`;
  }, [view, loading, listed, liveCount]);

  return (
    <AppFrame
      detailsOpen={active !== null && detailsOpen}
      sidebar={
        <Sidebar
          collapsed={railCollapsed}
          onToggleCollapsed={toggleRail}
          onCreate={() => {
            setChoosing(true);
          }}
          onSearch={() => {
            setCommandOpen(true);
          }}
          status={note === undefined ? showing : `${showing} · ${note}`}
          operationsActive={view === 'operations'}
          onOperations={
            canOperate(user.capabilities)
              ? () => {
                  setView(view === 'operations' ? 'sessions' : 'operations');
                }
              : undefined
          }
          account={
            <ProfileMenu
              user={user}
              onSignOut={onSignOut}
              onTerminalThemeChange={() => {
                setPaletteEpoch((n) => n + 1);
              }}
              onManageEnvironments={() => {
                setManagingProfiles(true);
              }}
            />
          }
        >
          {railCollapsed ? null : (
            <SessionRail
              sessions={listed}
              activeId={activeId}
              order={order}
              onSelect={select}
              onReorder={setOrder}
              arrangement={arrangementShown}
              onArrange={changeArrangement}
              stages={stages}
              waitingId={waitingId}
            />
          )}
        </Sidebar>
      }
      details={
        active === null ? null : (
          <SessionDrawer
            session={active}
            ending={endings[active.id]}
            busy={acting}
            onClose={() => {
              setDetailsOpen(false);
            }}
            onStop={() => void endSession(active.id)}
            onDelete={() => void removeSession(active.id)}
          />
        )
      }
      center={
        view === 'operations' ? (
          <OperatorPage store={operatorStore} capabilities={user.capabilities} />
        ) : (
          <main className="console__surface">
            <SessionHeader
              session={active}
              waiting={active !== null && active.id === waitingId}
              kernels={kernels}
              onChangePlane={
                active === null ? undefined : (to) => void changeSessionPlane(active.id, to)
              }
              actions={
                active === null ? undefined : (
                  <SessionActions
                    session={active}
                    busy={acting}
                    onReconnect={() => {
                      /*
                       * A new driver for this session and no other. The surface
                       * holding the old one releases its transport and the new
                       * one dials again, which asks the control plane to take
                       * the sandbox back under management.
                       *
                       * Scoped deliberately: this used to bump `paletteEpoch`,
                       * which is also the canvas's key, so one session's
                       * Reconnect rebuilt every node on the canvas — every
                       * scrollback cleared, every socket redialled.
                       */
                      setNotice(null);
                      reconnect(active.id);
                    }}
                    onRevive={() => void reviveSessionById(active.id)}
                    onStop={() => void endSession(active.id)}
                    onDetails={() => {
                      setDetailsOpen((open) => !open);
                    }}
                    detailsOpen={detailsOpen}
                    kernels={kernels}
                    onChangePlane={(to) => void changeSessionPlane(active.id, to)}
                  />
                )
              }
            />

            {/*
             * Above the surface, never in place of it (§12). A failure belongs
             * to one request about one session, and the surface below may hold a
             * dozen live terminals: rendering the sentence instead of it
             * unmounted every one of them, so a stop that came back 409 cost
             * eleven other sessions their connection and their scrollback.
             *
             * Beneath the session header rather than above it. §12 puts the band
             * "at the head of the column"; put literally first it would push the
             * header down and break the one line the three column headers share,
             * which §5 exists to hold. The surface it must stay out of the way of
             * is the terminal, and it does.
             */}
            {notice !== null ? (
              <div className="notice notice--fail console__notice" role="alert">
                <div className="console__noticebody">
                  <p className="notice__title">{notice.failed}</p>
                  {notice.unchanged === undefined ? null : (
                    <p className="notice__detail">{notice.unchanged}</p>
                  )}
                </div>
                {notice.code === undefined ? null : (
                  <span className="console__noticecode">{notice.code}</span>
                )}
                <button
                  type="button"
                  className="button button--ghost button--sm console__noticedismiss"
                  onClick={() => {
                    setNotice(null);
                  }}
                >
                  Dismiss
                </button>
              </div>
            ) : null}

            {loading ? (
              <p className="console__empty">Loading your sessions…</p>
            ) : arrangementShown === 'canvas' && canvasNodes.length > 0 ? (
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
                onOpenToReply={(id) => {
                  /*
                   * §13: the recap hands the reply to the surface that can
                   * hold a reading column — the list arrangement, with this
                   * session selected.
                   */
                  select(id);
                  changeArrangement('list');
                }}
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
            ) : active.plane === 'conversation' ? (
              /*
               * §18 and §1: the middle column is drawn from the session's own
               * granted set, not from a preference and not from a third view.
               * A plane is a property of a session the way an arrangement is a
               * property of a list — the column changes, and nothing else
               * does. There is no toggle here; moving a session between planes
               * is a deliberate act with a cost, and it is #128.
               */
              <ConversationSurface
                session={active}
                /*
                 * The session's own granted set, parsed once at the boundary
                 * by `toSession`. Re-deriving it here handed `toCapabilities`
                 * an already-parsed Session — which carries no `conforms_to`
                 * — so every capability came back false: no interrupt key, no
                 * steering tag, usage permanently "not reported", and the
                 * concurrency policy silently falling back to `reject`.
                 */
                capabilities={active.capabilities}
                onWaitingChange={(waiting) => {
                  setWaitingId(waiting ? active.id : null);
                }}
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
        )
      }
      overlay={
        <>
          {commandOpen ? (
            <CommandBar
              sessions={listed}
              canvasAvailable
              onClose={() => {
                setCommandOpen(false);
              }}
              onSelectSession={(id) => {
                setView('sessions');
                select(id);
              }}
              onArrange={(next) => {
                setView('sessions');
                changeArrangement(next);
              }}
              onNewSession={() => {
                setChoosing(true);
              }}
            />
          ) : null}

          <NewSessionDialog
            open={choosing}
            profiles={profiles}
            kernels={kernels}
            store={profileStore}
            onCancel={() => {
              setChoosing(false);
            }}
            onProfileCreated={(created) => {
              // Added here rather than re-read: it is selectable for the next
              // session immediately, and the list this console holds is the one
              // the dialog will be given again.
              setProfiles((previous) => [created, ...previous]);
            }}
            onStart={(environmentProfile) => {
              setChoosing(false);
              void start(environmentProfile);
            }}
          />

          {managingProfiles ? (
            <EnvironmentProfiles
              store={profileStore}
              kernels={kernels}
              onClose={() => {
                setManagingProfiles(false);
                // Reread on close rather than on every render: what someone just
                // authored has to be selectable in the next session they start,
                // and this is the only moment it can have changed.
                void profileStore
                  .list()
                  .then(setProfiles)
                  .catch(() => {
                    // Already reported inside the manager; failing to refresh the
                    // selector is not worth a second message about the same thing.
                  });
              }}
            />
          ) : null}
        </>
      }
    />
  );
}
