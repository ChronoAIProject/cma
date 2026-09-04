import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { SessionRail } from './sessions/SessionRail';
import { SessionDrawer } from './sessions/SessionDrawer';
import { AppFrame } from './shell/AppFrame';
import { Sidebar } from './shell/Sidebar';
import type { SortOrder } from './sessions/sessionTime';
import type { KernelOffer, Session, SessionLifecycle } from './sessions/types';
import { NewSessionDialog } from './sessions/NewSessionDialog';
import { StartupProgress } from './sessions/StartupProgress';
import { BouncingText } from './sessions/BouncingText';
import { SignInCard } from './chat/SignInCard';
import type { SignIn } from '@chronoai/cma-sdk';
import type { Stage } from './sessions/progress';
import { ChatSessionActions, SessionActions } from './sessions/SessionActions';
import { SessionHeader } from './sessions/SessionHeader';
import { TerminalSurface } from './terminal/TerminalSurface';
import { ConversationSurface } from './chat/ConversationSurface';
import { ChatSurface } from './chat/ChatSurface';
import { ChatProvisioning } from './chat/ChatProvisioning';
import { useSessionDrivers } from './sessions/drivers';
import { SessionCanvas } from './canvas/SessionCanvas';
import { selectLiveWindow, unattachedNote } from './canvas/layout';
import { loadArrangement, saveArrangement } from './sessions/arrangement';
import type { Arrangement } from './sessions/arrangement';
import { ProfileMenu } from './profile/ProfileMenu';
import { CommandBar } from './shell/CommandBar';
import { consumePendingTurn } from './auth/pendingTurn';
import type { User } from './auth/types';
import {
  ApiError,
  createChatSession,
  followSession,
  listKernels,
  listSessions,
  quoteSession,
  readAllowance,
  readChatSurfaceEnabled,
  readSessionResources,
  readSessionSpend,
  resetChatEnvironment as requestChatEnvironmentReset,
} from './api/client';
import {
  abilitiesFor,
  creationAbilityFor,
  listable,
  type Allowance,
} from './sessions/availability';
import { useSessionActions, type FailureNotice } from './sessions/useSessionActions';
import { useSessionRefresh } from './sessions/useSessionRefresh';
import { EnvironmentProfiles } from './environment/EnvironmentProfiles';
import { createRestProfileStore } from './environment/rest';
import type { EnvironmentProfile, EnvironmentProfileStore } from './environment/types';
import { OperatorPage } from './operator/OperatorPage';
import { canOperate } from './operator/access';
import { createRestOperatorStore } from './operator/rest';
import type { OperatorStore } from './operator/types';
import { DeveloperSurface } from './developer/DeveloperSurface';
import './Console.css';
import { DetailsPanel } from './chat/DetailsPanel';
import { sectionsOf } from './chat/ToolCall';
import type { ToolCallEntry } from '@chronoai/cma-sdk';
import type { AppRoute, AppRouter } from './routing/routes';
import { TriggerReview } from './trigger/TriggerReview';
import { LaunchSurface } from './trigger/LaunchSurface';
import { ContributionSurface } from './trigger/ContributionSurface';
import { PublisherSurface } from './publisher/PublisherSurface';
import { loadSurfaceMode, storeSurfaceMode, type SurfaceMode } from './shell/surfaceMode';

/**
 * The authenticated surface. Same page, now driven live.
 *
 * Utility views stay inside one page frame, per docs/Web_Style.md §1. The
 * canonical Trigger routes select workflow surfaces; account actions choose
 * between transient utility views without adding URLs for them.
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
  readonly route?: AppRoute;
  readonly onNavigate?: AppRouter['navigate'];
}

const CHAT_CREATE_GATEWAY_TIMEOUT = 524;
const CHAT_CREATE_RECOVERY_WINDOW_MS = 90_000;
const CHAT_CREATE_RECOVERY_POLL_MS = 2_000;

type PendingChatCreation =
  | { readonly kind: 'preparing'; readonly message: string }
  | { readonly kind: 'failed'; readonly notice: FailureNotice };

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, milliseconds);
  });
}

function omitSessionIds<T>(
  values: Readonly<Record<string, T>>,
  ids: ReadonlySet<string>,
): Record<string, T> {
  return Object.fromEntries(Object.entries(values).filter(([id]) => !ids.has(id)));
}

/**
 * Cloudflare can stop waiting before a cold Chat host finishes provisioning,
 * while the admitted origin request continues and durably creates its thread.
 * Reconcile that one ambiguous gateway outcome against the owner's Chat list
 * so the UI never reports that nothing was created when the thread exists.
 */
async function recoverTimedOutChat(knownIds: ReadonlySet<string>): Promise<Session | null> {
  const deadline = Date.now() + CHAT_CREATE_RECOVERY_WINDOW_MS;
  while (Date.now() < deadline) {
    try {
      const chats = await listSessions('chat');
      const created = chats.find((chat) => !knownIds.has(chat.id));
      if (created !== undefined) {
        return created;
      }
    } catch {
      // The original 524 remains the useful error if reconciliation also fails.
    }
    await wait(CHAT_CREATE_RECOVERY_POLL_MS);
  }
  return null;
}

/** The transient surfaces §1 admits. They are state, never routes. */
type ConsoleView = 'sessions' | 'publisher' | 'operations' | 'developer';

function belongsToSurface(session: Session, surface: SurfaceMode): boolean {
  return surface === 'chat' ? session.chat !== undefined : session.chat === undefined;
}

/**
 * Geometry the first frame is requested at.
 *
 * The surface reports its real size as soon as it measures, and the driver
 * holds any resize until the relay is ready, so this is only ever the opening
 * request rather than a size the terminal is stuck with.
 */

export function Console({
  user,
  onSignOut,
  route = { kind: 'home' },
  onNavigate = () => {},
}: ConsoleProps) {
  /*
   * Read the landing-page prompt exactly once, on first render of the
   * authenticated surface. consumePendingTurn clears it as it reads, so a
   * refresh cannot resubmit the visitor's text a second time.
   */
  const [openingTurn] = useState<string | null>(() => consumePendingTurn());
  const [openingForId, setOpeningForId] = useState<string | null>(null);

  const [sessions, setSessions] = useState<readonly Session[]>([]);
  const [surfaceMode, setSurfaceMode] = useState<SurfaceMode>(loadSurfaceMode);
  const [chatAvailable, setChatAvailable] = useState(false);
  const [allowance, setAllowance] = useState<Allowance | undefined>(undefined);
  const allowanceRead = useRef(0);
  const [activeId, setActiveId] = useState<string | null>(null);
  const routeRef = useRef(route);
  const [notice, setNotice] = useState<FailureNotice | null>(null);
  const [pendingChatCreation, setPendingChatCreation] = useState<PendingChatCreation | null>(null);
  const chatCreationInFlight = useRef(false);
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
  // Updated synchronously with the visual busy state. A provisioning stream
  // can close while its owner is deliberately deleting that session; reporting
  // that expected close before the DELETE settles produces a false error flash.
  const actingRef = useRef(false);
  const markActing = useCallback((next: boolean) => {
    actingRef.current = next;
    setActing(next);
  }, []);
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
   * Which utility view from §1 is showing. This state is outside the canonical
   * workflow router and is deliberately not persisted; see above.
   */
  const [view, setView] = useState<ConsoleView>('sessions');

  const toggleUtilityView = useCallback(
    (next: Exclude<ConsoleView, 'sessions'>) => {
      const opening = view !== next;
      if (opening && route.kind !== 'home') {
        onNavigate({ kind: 'home' }, { replace: true });
      }
      setView(opening ? next : 'sessions');
    },
    [onNavigate, route.kind, view],
  );

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
  /*
   * Sign-ins a session is waiting on, by id. Held here rather than in the
   * conversation, because a kernel raises this from inside the conversation
   * open — so the session is still provisioning and its conversation surface
   * has not been drawn yet. This is the only state where the request exists
   * and the surface that renders it does not.
   */
  const [signIns, setSignIns] = useState<Record<string, SignIn>>({});
  const [order, setOrder] = useState<SortOrder>('newest');
  /*
   * The details drawer — §12: closed by default, opened from the header's
   * Details key. Not persisted: it is reference material opened on demand,
   * not a layout choice to carry across visits.
   */
  const [detailsOpen, setDetailsOpen] = useState(false);
  /**
   * The tool call the details column is showing, when it is showing one.
   *
   * Held here rather than in the conversation surface because the column
   * belongs to the page: the surface reports what was asked for, and the page
   * decides what that column draws. `conversationModel` is shape-only by its
   * own contract, so it holds none of this either.
   */
  const [inspecting, setInspecting] = useState<ToolCallEntry | null>(null);
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

  const selectedSession = sessions.find((session) => session.id === activeId) ?? null;
  const routedSession =
    route.kind === 'session'
      ? (sessions.find((session) => session.id === route.sessionId) ?? null)
      : null;
  const active = route.kind === 'session' ? routedSession : selectedSession;
  const effectiveActiveId = active?.id ?? null;
  const effectiveFocusHistory = useMemo(
    () =>
      effectiveActiveId === null || focusHistory[0] === effectiveActiveId
        ? focusHistory
        : [effectiveActiveId, ...focusHistory.filter((seen) => seen !== effectiveActiveId)],
    [effectiveActiveId, focusHistory],
  );
  // Bound once so the surface below can be told it exists. Read straight from
  // the map, `noUncheckedIndexedAccess` makes every use `SignIn | undefined`.
  const pendingSignIn = active === null ? undefined : signIns[active.id];

  /** Select a session, in whichever arrangement is showing. */
  const selectState = useCallback((id: string) => {
    setPendingChatCreation(null);
    setActiveId(id);
    /*
     * Choosing a session is asking to look at it, so it leaves whatever else
     * was showing. Without this, picking one from the rail while Operations
     * was open changed which session was active and kept the operator panel
     * on screen — the only way back was to press Operations again.
     *
     * Here rather than at each call site: the command bar already did this
     * and the rail did not, which is the same intent written twice and only
     * half carried.
     */
    setView('sessions');
    /*
     * Whatever the details column was inspecting belonged to the session
     * being left. Carrying it across would leave a panel describing a call
     * from a conversation that is no longer on screen.
     */
    setInspecting(null);
    setFocusHistory((prev) =>
      prev[0] === id ? prev : [id, ...prev.filter((seen) => seen !== id)],
    );
  }, []);

  const select = useCallback(
    (id: string) => {
      selectState(id);
      onNavigate({ kind: 'session', sessionId: id });
    },
    [onNavigate, selectState],
  );

  const clearSelection = useCallback(() => {
    setPendingChatCreation(null);
    setActiveId(null);
    onNavigate({ kind: 'home' }, { replace: true });
  }, [onNavigate]);

  useEffect(() => {
    routeRef.current = route;
  }, [route]);

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

  const refreshSession = useSessionRefresh(setSessions);

  const refreshAllowance = useCallback(() => {
    const read = ++allowanceRead.current;
    void readAllowance()
      .then((current) => {
        if (read === allowanceRead.current) {
          setAllowance(current);
        }
      })
      .catch(() => {
        if (read === allowanceRead.current) {
          // Advisory only. A failed read leaves creation enabled.
          setAllowance(undefined);
        }
      });
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
        const chatEnabled = await readChatSurfaceEnabled();
        if (cancelled) {
          return;
        }
        const agents = await listSessions('agent');
        let chats: readonly Session[] = [];
        if (chatEnabled) {
          try {
            chats = await listSessions('chat');
          } catch (cause) {
            if (!cancelled) {
              report(
                cause,
                'Loading your chats failed.',
                'Your Agent sessions are still available.',
              );
            }
          }
        }
        const existing = [...agents, ...chats];
        if (cancelled) {
          return;
        }
        setChatAvailable(chatEnabled);
        setSessions(existing);
        const openingRoute = routeRef.current;
        const routed =
          openingRoute.kind === 'session'
            ? existing.find((session) => session.id === openingRoute.sessionId)
            : undefined;
        const preferredSurface = chatEnabled ? surfaceMode : 'agent';
        const requested =
          routed ??
          existing.find(
            (session) =>
              belongsToSurface(session, preferredSurface) && session.lifecycle !== 'terminated',
          );
        if (requested !== undefined) {
          const requestedSurface = requested.chat === undefined ? 'agent' : 'chat';
          if (requestedSurface === 'agent' || chatEnabled) {
            setSurfaceMode(requestedSurface);
          }
          selectState(requested.id);
        }
      } catch (cause) {
        if (!cancelled) {
          report(cause, 'Loading your sessions failed.', 'Nothing was changed.');
        }
      } finally {
        if (!cancelled) {
          refreshAllowance();
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

  const {
    start,
    endSession,
    terminateSessionById,
    reviveSessionById,
    removeSession,
    changeSessionPlane,
  } = useSessionActions({
    sessions,
    activeId: effectiveActiveId,
    setSessions,
    clearSelection,
    setOpeningForId,
    setNotice,
    setActing: markActing,
    select,
    report,
    applyLifecycle,
    setFocusHistory,
    refreshAllowance,
  });

  const startChat = useCallback(async () => {
    if (chatCreationInFlight.current) {
      return;
    }
    chatCreationInFlight.current = true;
    setNotice(null);
    markActing(true);
    setSurfaceMode('chat');
    storeSurfaceMode('chat');
    setView('sessions');
    setActiveId(null);
    setDetailsOpen(false);
    setInspecting(null);
    setPendingChatCreation({
      kind: 'preparing',
      message: 'Setting up the shared sandbox and a fresh workspace.',
    });
    if (route.kind !== 'home') {
      onNavigate({ kind: 'home' }, { replace: true });
    }
    const knownChatIds = new Set(
      sessions.filter((session) => session.chat !== undefined).map((session) => session.id),
    );
    try {
      const session = await createChatSession();
      setSessions((previous) => [session, ...previous]);
      select(session.id);
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === CHAT_CREATE_GATEWAY_TIMEOUT) {
        setPendingChatCreation({
          kind: 'preparing',
          message: 'The sandbox is taking longer than usual. Checking for your new Chat.',
        });
        const recovered = await recoverTimedOutChat(knownChatIds);
        if (recovered !== null) {
          setSessions((previous) => [
            recovered,
            ...previous.filter((session) => session.id !== recovered.id),
          ]);
          select(recovered.id);
          return;
        }
      }
      if (cause instanceof ApiError) {
        setPendingChatCreation({
          kind: 'failed',
          notice: {
            failed: cause.message,
            unchanged:
              cause.status === CHAT_CREATE_GATEWAY_TIMEOUT
                ? 'The timed-out request could not be reconciled with your Chats.'
                : 'No Chat was created.',
            code: cause.code ?? (cause.status > 0 ? `HTTP ${String(cause.status)}` : undefined),
          },
        });
      } else {
        setPendingChatCreation({
          kind: 'failed',
          notice: {
            failed: `Starting a Chat failed. ${String(cause)}`,
            unchanged: 'No Chat was created.',
          },
        });
      }
    } finally {
      chatCreationInFlight.current = false;
      markActing(false);
    }
  }, [markActing, onNavigate, route.kind, select, sessions]);

  const resetChatEnvironment = useCallback(async () => {
    const chatIds = new Set(
      sessions.filter((session) => session.chat !== undefined).map((session) => session.id),
    );
    setNotice(null);
    markActing(true);
    try {
      try {
        await requestChatEnvironmentReset();
      } catch (cause) {
        try {
          const remaining = await listSessions('chat');
          if (remaining.length > 0) {
            report(
              cause,
              'Resetting the Chat environment failed.',
              'Your Chats and shared sandbox remain available.',
            );
            return;
          }
        } catch {
          report(
            cause,
            'Resetting the Chat environment failed.',
            'Its current state could not be confirmed. Refresh before trying again.',
          );
          return;
        }
      }

      setSessions((previous) => previous.filter((session) => session.chat === undefined));
      setFocusHistory((previous) => previous.filter((id) => !chatIds.has(id)));
      setEndings((previous) => omitSessionIds(previous, chatIds));
      setStages((previous) => omitSessionIds(previous, chatIds));
      setSignIns((previous) => omitSessionIds(previous, chatIds));
      setWaitingId((current) => (current !== null && chatIds.has(current) ? null : current));
      setOpeningForId((current) => (current !== null && chatIds.has(current) ? null : current));
      setInspecting(null);
      setDetailsOpen(false);
      clearSelection();
    } finally {
      refreshAllowance();
      markActing(false);
    }
  }, [clearSelection, markActing, refreshAllowance, report, sessions]);

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
        // Settled: everything provisioning fills is now written down, and this
        // client is still holding what it was handed before it ran.
        if (lifecycle === 'ready' || lifecycle === 'running') {
          refreshSession(id);
        }
        if (detail !== undefined && detail !== '') {
          setEndings((prev) => ({ ...prev, [id]: detail }));
        }
        // Leaving `provisioning` is the sign-in having been completed, or
        // having stopped mattering. Either way the prompt is stale, and a
        // stale one asks somebody to enter a code that has already expired.
        if (lifecycle !== 'provisioning') {
          setSignIns((prev) => {
            if (!(id in prev)) {
              return prev;
            }
            const rest = { ...prev };
            delete rest[id];
            return rest;
          });
        }
      },
      (message) => {
        if (!actingRef.current) {
          setNotice({ failed: message });
        }
      },
      (stage) => {
        setStages((prev) => ({ ...prev, [id]: stage }));
      },
      (signIn) => {
        setSignIns((prev) => ({ ...prev, [id]: signIn }));
      },
    );
    return stop;
    // Keyed on the session rather than the object: `active` is rebuilt on
    // every render, and depending on it would tear the stream down and reopen
    // it continuously.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id, active?.lifecycle, applyLifecycle, refreshSession]);

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
  const driver = nodes.find((node) => node.session.id === effectiveActiveId)?.driver ?? null;

  // The canvas shows exactly what the rail lists, so one arrangement never
  // holds a session the other has let go of.
  const effectiveSurface: SurfaceMode = chatAvailable ? surfaceMode : 'agent';
  const allListed = useMemo(() => listable(sessions), [sessions]);
  const listed = useMemo(
    () => allListed.filter((session) => belongsToSurface(session, effectiveSurface)),
    [allListed, effectiveSurface],
  );
  const canvasNodes = useMemo(
    () =>
      nodes.filter(
        (node) =>
          abilitiesFor(node.session).listed && belongsToSurface(node.session, effectiveSurface),
      ),
    [nodes, effectiveSurface],
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
    return unattachedNote(
      connectable.length,
      selectLiveWindow(connectable, effectiveFocusHistory).size,
    );
  }, [view, arrangementShown, canvasNodes, effectiveFocusHistory]);

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
    if (view === 'developer') {
      return 'Developer';
    }
    if (view === 'publisher') {
      return 'GitHub Triggers';
    }
    if (loading) {
      return 'Loading sessions';
    }
    if (listed.length === 0) {
      return effectiveSurface === 'chat' ? 'No chats' : 'No sessions';
    }
    const noun = effectiveSurface === 'chat' ? 'chat' : 'session';
    const count = `${String(listed.length)} ${noun}${listed.length === 1 ? '' : 's'}`;
    return `${count} · ${String(liveCount)} live`;
  }, [view, loading, listed, liveCount, effectiveSurface]);
  const agentCreation = useMemo(() => creationAbilityFor(allowance), [allowance]);
  const creation =
    effectiveSurface === 'chat'
      ? acting || pendingChatCreation?.kind === 'preparing'
        ? { canCreate: false, reason: 'A new Chat is already being prepared.' }
        : { canCreate: true }
      : agentCreation;

  const changeSurface = useCallback(
    (next: SurfaceMode) => {
      setSurfaceMode(next);
      storeSurfaceMode(next);
      const candidates = listable(sessions).filter((session) => belongsToSurface(session, next));
      const current = candidates.find((session) => session.id === effectiveActiveId);
      const target =
        current ??
        candidates.find((session) => session.lifecycle !== 'terminated') ??
        candidates[0];
      if (target === undefined) {
        clearSelection();
      } else {
        select(target.id);
      }
    },
    [clearSelection, effectiveActiveId, select, sessions],
  );

  const beginCreate = useCallback(() => {
    if (effectiveSurface === 'chat') {
      void startChat();
    } else {
      setChoosing(true);
    }
  }, [effectiveSurface, startChat]);

  return (
    <AppFrame
      detailsOpen={
        (route.kind === 'home' || route.kind === 'session') &&
        view === 'sessions' &&
        pendingChatCreation === null &&
        active !== null &&
        detailsOpen
      }
      sidebarCollapsed={railCollapsed}
      onSidebarCollapsedChange={setRailCollapsed}
      sidebar={({ collapsed, onToggle }) => (
        <Sidebar
          collapsed={collapsed}
          creation={creation}
          surface={effectiveSurface}
          chatAvailable={chatAvailable}
          onSurfaceChange={changeSurface}
          onToggleCollapsed={onToggle}
          onCreate={beginCreate}
          onSearch={() => {
            setCommandOpen(true);
          }}
          /*
           * Only what the list cannot say for itself. An empty list already
           * reads "No sessions yet."; a status line under it repeating "No
           * sessions" is the same fact twice, one line apart.
           */
          status={
            listed.length === 0 ? undefined : note === undefined ? showing : `${showing} · ${note}`
          }
          account={
            <ProfileMenu
              user={user}
              onSignOut={onSignOut}
              onManageEnvironments={() => {
                setManagingProfiles(true);
              }}
              collapsed={collapsed}
              operationsActive={view === 'operations'}
              developerActive={view === 'developer'}
              publisherActive={view === 'publisher'}
              onPublisher={() => {
                toggleUtilityView('publisher');
              }}
              onDeveloper={() => {
                toggleUtilityView('developer');
              }}
              onOperations={
                canOperate(user.capabilities)
                  ? () => {
                      toggleUtilityView('operations');
                    }
                  : undefined
              }
            />
          }
        >
          {collapsed ? null : (
            <SessionRail
              sessions={listed}
              surface={effectiveSurface}
              activeId={pendingChatCreation === null ? effectiveActiveId : null}
              order={order}
              onSelect={select}
              onReorder={setOrder}
              arrangement={arrangementShown}
              onArrange={changeArrangement}
              loading={loading}
              stages={stages}
              waitingId={waitingId}
              signingIn={signIns}
            />
          )}
        </Sidebar>
      )}
      details={
        /*
         * One column, two occupants. A tool call takes it over while one is
         * being inspected — they answer different questions about the same
         * session, and two columns would leave the second one empty most of
         * the time.
         */
        pendingChatCreation !== null ? null : inspecting !== null ? (
          <DetailsPanel
            title={inspecting.target ?? inspecting.toolKind}
            sections={sectionsOf(inspecting)}
            onClose={() => {
              setInspecting(null);
            }}
          />
        ) : active === null ? null : (
          <SessionDrawer
            session={active}
            ending={endings[active.id]}
            readSpend={readSessionSpend}
            readResources={readSessionResources}
            open={
              (route.kind === 'home' || route.kind === 'session') &&
              view === 'sessions' &&
              detailsOpen
            }
            navigate={onNavigate}
            onClose={() => {
              setDetailsOpen(false);
            }}
          />
        )
      }
      center={
        pendingChatCreation !== null ? (
          <main className="console__surface">
            <SessionHeader
              session={null}
              emptyLabel={
                pendingChatCreation.kind === 'preparing'
                  ? 'New Chat / Preparing'
                  : 'New Chat / Needs attention'
              }
            />
            <ChatProvisioning state={pendingChatCreation} onRetry={() => void startChat()} />
          </main>
        ) : route.kind === 'trigger' ? (
          <TriggerReview
            key={route.publicId}
            ownerId={user.id}
            publicId={route.publicId}
            authError={route.authError}
            navigate={onNavigate}
          />
        ) : route.kind === 'trigger_recovery' ? (
          <TriggerReview
            key={route.launchId}
            ownerId={user.id}
            recoveryLaunchId={route.launchId}
            navigate={onNavigate}
          />
        ) : route.kind === 'launch' ? (
          <LaunchSurface
            key={route.launchId}
            ownerId={user.id}
            launchId={route.launchId}
            navigate={onNavigate}
          />
        ) : route.kind === 'contribute' ? (
          <ContributionSurface
            key={route.launchId}
            ownerId={user.id}
            launchId={route.launchId}
            navigate={onNavigate}
          />
        ) : route.kind === 'session' && !loading && routedSession === null ? (
          <main className="console__surface">
            <div className="console__empty" role="alert">
              This session does not exist or is not available to this account.
            </div>
          </main>
        ) : route.kind === 'not_found' ? (
          <main className="console__surface">
            <div className="console__empty" role="alert">
              This CMA page does not exist.
            </div>
          </main>
        ) : view === 'operations' ? (
          <OperatorPage store={operatorStore} capabilities={user.capabilities} />
        ) : view === 'developer' ? (
          <DeveloperSurface />
        ) : view === 'publisher' ? (
          <PublisherSurface ownerId={user.id} displayIdentity={user.displayName ?? user.handle} />
        ) : (
          <main className="console__surface">
            <SessionHeader
              session={active}
              waiting={active !== null && active.id === waitingId}
              actions={
                active === null ? undefined : active.chat === undefined ? (
                  <SessionActions
                    session={active}
                    signingIn={pendingSignIn !== undefined}
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
                    onTerminate={() => void terminateSessionById(active.id)}
                    onDelete={() => void removeSession(active.id)}
                    onTerminalThemeChange={() => {
                      setPaletteEpoch((n) => n + 1);
                    }}
                    onDetails={() => {
                      setDetailsOpen((open) => !open);
                    }}
                    detailsOpen={detailsOpen}
                    kernels={kernels}
                    onChangePlane={(to) => void changeSessionPlane(active.id, to)}
                  />
                ) : (
                  <ChatSessionActions
                    busy={acting}
                    onDelete={() => void removeSession(active.id)}
                    onResetEnvironment={() => void resetChatEnvironment()}
                    onDetails={() => {
                      setDetailsOpen((open) => !open);
                    }}
                    detailsOpen={detailsOpen}
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
              // §14a: the wave says the surface is live. A bar would claim to
              // know how far along a read is, and nothing here does.
              <p className="console__empty">
                <BouncingText text="Reading your sessions" />
              </p>
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
                focusedId={effectiveActiveId}
                focusHistory={effectiveFocusHistory}
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
            ) : active.lifecycle === 'provisioning' && pendingSignIn !== undefined ? (
              /*
               * Ahead of the progress, not beside it. The session is not
               * starting and no amount of waiting will start it — the agent
               * has stopped to ask for something that has to be done on
               * another device. Leaving the "this can take a minute" message
               * on screen would be telling somebody to wait for the thing
               * they are being asked to do.
               */
              <SignInCard signIn={pendingSignIn} />
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
            ) : active.chat !== undefined ? (
              <ChatSurface key={active.id} session={active} />
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
                /*
                 * Keyed on the session, which `ConversationSurface` requires
                 * of its callers: it folds events into state it holds for as
                 * long as it is mounted and never clears them, so a reused
                 * component folds the next session's turns on top of the last
                 * one's. `SessionCanvas` keys its nodes for the same reason;
                 * this arrangement did not, and the two sessions ran together.
                 *
                 * Only this surface. Keying the column would remount the
                 * terminal with it, clearing its scrollback and redialling its
                 * socket on every switch.
                 */
                key={active.id}
                session={active}
                /*
                 * The session's own granted set, parsed once at the boundary
                 * by `toSession`. Re-deriving it here handed `toCapabilities`
                 * an already-parsed Session — which carries no raw grant list
                 * — so every capability came back false: no interrupt key, no
                 * steering tag, usage permanently "not reported", and the
                 * concurrency policy silently falling back to `reject`.
                 */
                capabilities={active.capabilities}
                suppressConnectionFailure={acting || active.lifecycle === 'terminated'}
                onWaitingChange={(waiting) => {
                  setWaitingId(waiting ? active.id : null);
                }}
                inspectingId={inspecting?.id}
                onInspect={(call) => {
                  setInspecting(call);
                  setDetailsOpen(true);
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
              surface={effectiveSurface}
              creation={creation}
              canvasAvailable
              onClose={() => {
                setCommandOpen(false);
              }}
              onSelectSession={(id) => {
                select(id);
              }}
              onArrange={(next) => {
                setView('sessions');
                changeArrangement(next);
              }}
              onNewSession={() => {
                beginCreate();
              }}
            />
          ) : null}

          <NewSessionDialog
            open={choosing && effectiveSurface === 'agent'}
            profiles={profiles}
            kernels={kernels}
            store={profileStore}
            quote={quoteSession}
            onCancel={() => {
              setChoosing(false);
            }}
            onProfileCreated={(created) => {
              // Added here rather than re-read: it is selectable for the next
              // session immediately, and the list this console holds is the one
              // the dialog will be given again.
              setProfiles((previous) => [created, ...previous]);
            }}
            onStart={async (environmentProfile, quotedRateCard) => {
              await start(environmentProfile, quotedRateCard);
              setChoosing(false);
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
