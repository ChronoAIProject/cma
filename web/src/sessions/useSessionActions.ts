import { useCallback } from 'react';
import { createSession, deleteSession, reviveSession, stopSession } from '../api/client';
import type { AgentOptions } from './options';
import type { Kernel, Session, SessionLifecycle } from './types';

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
  readonly setNotice: (notice: string | null) => void;
  readonly setActing: (acting: boolean) => void;
  readonly select: (id: string) => void;
  readonly report: (cause: unknown) => void;
  readonly applyLifecycle: (id: string, lifecycle: SessionLifecycle) => void;
  readonly setFocusHistory: React.Dispatch<React.SetStateAction<readonly string[]>>;
}

export interface SessionActions {
  /** Start a session and put it at the head of the list. */
  readonly start: (
    kernel: Kernel,
    options: AgentOptions,
    environmentProfile?: string | null,
  ) => Promise<void>;
  /** Release the sandbox, keeping the session readable and revivable. */
  readonly endSession: (id: string) => Promise<void>;
  /** Give a stopped session a sandbox again, under the same identifier. */
  readonly reviveSessionById: (id: string) => Promise<void>;
  /** Remove the session and its history. */
  readonly removeSession: (id: string) => Promise<void>;
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
}: SessionActionHandles): SessionActions {
  const start = useCallback(
    async (kernel: Kernel, options: AgentOptions, environmentProfile: string | null = null) => {
      setNotice(null);
      try {
        // Returns as soon as the session exists, before its runtime does, so the
        // tab appears immediately and reports its own progress.
        const session = await createSession(
          kernel,
          INITIAL_COLS,
          INITIAL_ROWS,
          options,
          environmentProfile,
        );
        setSessions((prev) => [session, ...prev]);
        select(session.id);
        setOpeningForId((prev) => prev ?? session.id);
      } catch (cause) {
        report(cause);
      }
    },
    [report, select, setNotice, setOpeningForId, setSessions],
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
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [applyLifecycle, report, setActing, setNotice],
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
        report(cause);
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
        report(cause);
      } finally {
        setActing(false);
      }
    },
    [
      activeId,
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

  return { start, endSession, reviveSessionById, removeSession };
}
