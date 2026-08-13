import { useEffect, useMemo, useState } from 'react';
import type { Session } from './types';
import type { LiveSession } from './live';
import type { TerminalDriver } from '../terminal/types';
import { abilitiesFor } from './availability';
import { createWebSocketDriver } from '../terminal/drivers/websocket';
import { createTranscriptDriver } from '../terminal/drivers/transcript';
import { terminalUrl } from '../api/client';

/**
 * One driver per session, built from the session's own state.
 *
 * Both views need this and they need the same answer. Tabs mode shows one
 * terminal and the canvas shows many, but "what does this session's terminal
 * read from" is a property of the session rather than of the arrangement, so it
 * is decided once, here, and neither view gets a say.
 *
 * Which driver a session gets follows docs/Web_Style.md §14 through
 * `abilitiesFor`, and nothing here re-derives that rule. A session that can be
 * typed into gets the relay; one that has stopped or ended gets its stored
 * transcript, read-only; one that is still starting or being moved gets
 * nothing, because there is no screen behind it yet.
 */

/** Where a session's terminal reads from, given what its state allows. */
export type DriverKind = 'live' | 'transcript' | 'none';

export function driverKindFor(session: Session): DriverKind {
  const can = abilitiesFor(session);
  if (can.interactive) {
    return 'live';
  }
  /*
   * Stopping a session releases what it costs; it does not hide what it did.
   * Ended sessions are the same — reading the last thing a failed session said
   * is usually the whole reason its owner opened it.
   */
  return can.canRevive || session.lifecycle === 'terminated' ? 'transcript' : 'none';
}

/** The opening prompt a visitor typed before signing in, and where it belongs. */
export interface OpeningTurn {
  readonly text: string;
  /** The session that inherits it — the first one this console opened. */
  readonly sessionId: string;
}

/** What the registry needs a driver for. */
export interface DriverRequest {
  readonly session: Session;
  readonly kind: 'live' | 'transcript';
  /** Present only for the one session that inherits the visitor's prompt. */
  readonly openingTurn?: string;
}

/**
 * How a driver is made.
 *
 * Injected so the registry can be exercised without a transport, and so a
 * future driver — a replay, a recording — plugs in without this file learning
 * about it.
 */
export type DriverFactory = (request: DriverRequest) => TerminalDriver;

function defaultFactory({ session, kind, openingTurn }: DriverRequest): TerminalDriver {
  return kind === 'live'
    ? createWebSocketDriver({ url: terminalUrl(session.id), openingTurn })
    : createTranscriptDriver(session.id, abilitiesFor(session).reason ?? '');
}

export interface SessionDriverRegistry {
  /**
   * Drivers for these sessions, reusing every one already built for a session
   * whose state still calls for the same kind.
   *
   * Idempotent: resolving the same sessions twice returns the same drivers and
   * builds nothing, which is what makes it safe to call while rendering.
   */
  resolve(sessions: readonly Session[], opening: OpeningTurn | null): readonly LiveSession[];
  /** Forget every session not in this set. */
  prune(ids: Iterable<string>): void;
}

interface Entry {
  /** What the driver was built for. A change here means a different driver. */
  readonly signature: string;
  readonly driver: TerminalDriver;
}

/**
 * Signature of the driver a session currently calls for.
 *
 * The reason is part of it because the transcript driver prints it as its
 * closing line, so an idle session and an ended one do not share a driver even
 * though they share a kind.
 *
 * The opening turn is deliberately not part of it. That prompt belongs to the
 * first driver a session is ever given, and treating a later one as grounds to
 * rebuild would reconnect a live socket for no reason. What keeps the prompt
 * off a driver built later — after a revive, or a reconnect — is
 * `openingDelivered` rather than anything in this signature.
 */
function signatureFor(session: Session, kind: DriverKind): string {
  return `${kind}:${abilitiesFor(session).reason ?? ''}`;
}

/**
 * Build and remember a driver per session, rebuilding one only when the session
 * asks for a different kind.
 *
 * Rebuilding on every lifecycle change would be simpler and wrong: `ready` and
 * `running` both mean "typed into over the relay", and tearing the socket down
 * between them would interrupt a session nobody asked to interrupt.
 *
 * Nothing is detached on the way out. A driver a surface is holding is detached
 * when that surface unmounts, and one no surface ever held never opened a
 * transport, so there is nothing to release.
 */
export function createSessionDriverRegistry(
  build: DriverFactory = defaultFactory,
): SessionDriverRegistry {
  const entries = new Map<string, Entry>();
  /*
   * Whether the visitor's opening prompt has already been handed to a driver.
   *
   * It belongs to the first driver its session is ever given, and a session
   * gets another one whenever it comes back from being stopped. Handing the
   * prompt over again would not repaint anything — the driver submits it — so
   * an agent that already answered the question would be asked it a second
   * time, minutes or hours later, with no one having typed anything.
   *
   * One flag rather than a set, because an opening turn names exactly one
   * session.
   */
  let openingDelivered = false;

  return {
    resolve(sessions, opening) {
      return sessions.map((session): LiveSession => {
        const kind = driverKindFor(session);
        if (kind === 'none') {
          return { session, driver: null };
        }

        const signature = signatureFor(session, kind);
        const held = entries.get(session.id);
        if (held !== undefined && held.signature === signature) {
          return { session, driver: held.driver };
        }

        const carriesOpening =
          !openingDelivered && opening !== null && opening.sessionId === session.id;
        const driver = build({
          session,
          kind,
          openingTurn: carriesOpening ? opening.text : undefined,
        });
        if (carriesOpening) {
          openingDelivered = true;
        }
        entries.set(session.id, { signature, driver });
        return { session, driver };
      });
    },

    prune(ids) {
      const alive = new Set(ids);
      for (const id of [...entries.keys()]) {
        if (!alive.has(id)) {
          entries.delete(id);
        }
      }
    },
  };
}

/** The registry, bound to a component's lifetime. */
export function useSessionDrivers(
  sessions: readonly Session[],
  opening: OpeningTurn | null,
): readonly LiveSession[] {
  // Created once and never replaced: the drivers it holds are the live
  // connections, and a registry rebuilt on a re-render would drop them all.
  const [registry] = useState(createSessionDriverRegistry);

  const nodes = useMemo(() => registry.resolve(sessions, opening), [registry, sessions, opening]);

  useEffect(() => {
    registry.prune(sessions.map((session) => session.id));
  }, [registry, sessions]);

  return nodes;
}
