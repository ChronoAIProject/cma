import { useCallback } from 'react';
import {
  changePlane,
  createSession,
  deleteSession,
  reviveSession,
  SessionLimitReached,
  stopSession,
} from '../api/client';
import type { Session, SessionLifecycle, SessionPlane } from './types';

/**
 * A failed action, as §12 reports it: what failed, what was not changed, and
 * the API's own code in identifier style. One at a time — the card above the
 * surface describes the most recent attempt, and every action clears it as
 * it starts.
 */
export interface FailureNotice {
  /** What failed, ending with the API's own words where it gave any. */
  readonly failed: string;
  /** What was not changed by the failure. */
  readonly unchanged?: string;
  /** The API's own code, in identifier style. */
  readonly code?: string;
}

/**
 * The geometry a session is asked for before a terminal has measured itself.
 *
 * A session has to be created with some size, and the surface that will render
 * it does not exist yet. It re-declares its real geometry the moment it mounts,
 * so this only decides what the agent draws against for its first instant.
 */
const INITIAL_COLS = 80;
const INITIAL_ROWS = 24;

/**
 * What the console can do to a session.
 *
 * Separated from the console because they are different subjects: this is what
 * happens to a session, and what remains there is how sessions are arranged on
 * screen. Neither reads better wrapped around the other, and the file was past
 * the size where both fit.
 *
 * The hook owns no state. Every piece it touches belongs to the console — the
 * session list, which one is selected, whether an action is in flight — so
 * every piece arrives as a handle. Holding a second copy here is how the rail
 * and the panel come to disagree about what exists.
 */

interface SessionActionHandles {
  readonly sessions: readonly Session[];
  readonly activeId: string | null;
  readonly setSessions: React.Dispatch<React.SetStateAction<readonly Session[]>>;
  readonly setActiveId: React.Dispatch<React.SetStateAction<string | null>>;
  readonly setOpeningForId: React.Dispatch<React.SetStateAction<string | null>>;
  readonly setNotice: (notice: FailureNotice | null) => void;
  readonly setActing: (acting: boolean) => void;
  readonly select: (id: string) => void;
  /** Report a failed attempt: the cause, what failed, what was not changed. */
  readonly report: (cause: unknown, failed: string, unchanged: string) => void;
  readonly applyLifecycle: (id: string, lifecycle: SessionLifecycle) => void;
  readonly setFocusHistory: React.Dispatch<React.SetStateAction<readonly string[]>>;
  /** Re-read the caller's advisory allowance after an action settles. */
  readonly refreshAllowance: () => void;
}

export interface SessionActions {
  /** Start a session and put it at the head of the list. */
  readonly start: (environmentProfile?: string | null) => Promise<void>;
  /** Release the sandbox, keeping the session readable and revivable. */
  readonly endSession: (id: string) => Promise<void>;
  /** Give a stopped session a sandbox again, under the same identifier. */
  readonly reviveSessionById: (id: string) => Promise<void>;
  /** Remove the session and its history. */
  readonly removeSession: (id: string) => Promise<void>;
  /** Move a session onto the other plane, which is a rotation. */
  readonly changeSessionPlane: (id: string, to: SessionPlane) => Promise<void>;
}

export function useSessionActions({
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
  refreshAllowance,
}: SessionActionHandles): SessionActions {
  const start = useCallback(
    async (environmentProfile: string | null = null) => {
      setNotice(null);
      try {
        // Returns as soon as the session exists, before its runtime does, so the
        // tab appears immediately and reports its own progress.
        const session = await createSession(INITIAL_COLS, INITIAL_ROWS, environmentProfile);
        setSessions((prev) => [session, ...prev]);
        select(session.id);
        setOpeningForId((prev) => prev ?? session.id);
      } catch (cause) {
        report(cause, 'Starting a session failed.', 'No session was created.');
      } finally {
        refreshAllowance();
      }
    },
    [refreshAllowance, report, select, setNotice, setOpeningForId, setSessions],
  );

  /** End the open session, leaving it readable. */
  const endSession = useCallback(
    async (id: string) => {
      // Cleared on the way in, as every action does, so the line above the
      // surface describes the attempt being watched rather than an older
      // failure sitting over a console that has since recovered.
      setNotice(null);
      setActing(true);
      try {
        // Stop rather than terminate: ending a session releases what it is
        // costing without throwing away what it did, and the owner can pick it
        // up again. Terminating is what delete does on the way out.
        await stopSession(id);
        applyLifecycle(id, 'idle');
      } catch (cause) {
        report(cause, 'Pausing the session failed.', 'The session keeps running.');
      } finally {
        refreshAllowance();
        setActing(false);
      }
    },
    [applyLifecycle, refreshAllowance, report, setActing, setNotice],
  );

  /** Give a stopped session a sandbox again, and follow it back up. */
  const reviveSessionById = useCallback(
    async (id: string) => {
      setNotice(null);
      setActing(true);
      try {
        const revived = await reviveSession(id);
        // Back to provisioning, which is what the effect below follows: the
        // progress of a revive is the progress of a start, because it is one.
        setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, ...revived } : s)));
      } catch (cause) {
        if (cause instanceof SessionLimitReached) {
          setNotice({
            failed: cause.message,
            unchanged: 'The session stays stopped.',
            code: 'session_limit_reached',
          });
        } else {
          report(cause, 'Reviving the session failed.', 'The session stays stopped.');
        }
      } finally {
        refreshAllowance();
        setActing(false);
      }
    },
    [refreshAllowance, report, setActing, setNotice, setSessions],
  );

  /**
   * Move a session onto the other plane.
   *
   * The session comes back `rotating` and finishes the move on its own, so the
   * record is replaced with what the control plane answered rather than
   * assumed: the plane, the granted set and the rotation count all changed,
   * and every one of them decides what the middle column draws next.
   *
   * A refusal is reported as it arrived. The control plane names the
   * capability that is missing, and that sentence is the whole value of a
   * typed refusal — §18.6 draws it rather than hiding it.
   */
  const changeSessionPlane = useCallback(
    async (id: string, to: SessionPlane) => {
      setNotice(null);
      setActing(true);
      try {
        const moving = await changePlane(id, to);
        setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, ...moving } : s)));
      } catch (cause) {
        report(cause, 'Moving the session failed.', 'The session stays on its plane.');
      } finally {
        setActing(false);
      }
    },
    [report, setActing, setNotice, setSessions],
  );

  /** Remove the session and its history. */
  const removeSession = useCallback(
    async (id: string) => {
      setNotice(null);
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
        report(cause, 'Deleting the session failed.', 'The session and its history are untouched.');
      } finally {
        refreshAllowance();
        setActing(false);
      }
    },
    [
      activeId,
      refreshAllowance,
      report,
      select,
      sessions,
      setActing,
      setActiveId,
      setFocusHistory,
      setNotice,
      setSessions,
    ],
  );

  return { start, endSession, reviveSessionById, removeSession, changeSessionPlane };
}
