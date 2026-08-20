/**
 * One client, configured once.
 *
 * The functions underneath all take a config as their first argument, which is
 * what makes two clients in one process possible. This is the shape a consumer
 * actually wants to hold: `chronoBot.listSessions()` rather than
 * `listSessions(config)` threaded through every call site.
 *
 * Both are exported. A React app holds the object; a test that wants one call
 * against a throwaway config uses the function directly and constructs nothing.
 */

import { resolveConfig, type ClientConfig, type ResolvedConfig } from './config';
import * as sessions from './sessions';
import type { CreateSession } from './sessions';
import type { KernelOffer, Session, SessionPlane } from './types';

/**
 * A configured chrono-bot client.
 *
 * Holds no session state and caches nothing. Every method is a request, and
 * what a session *is* comes back from the service rather than from anything
 * remembered here — which is the same rule the control plane holds itself to
 * (`CLAUDE.md` §5: the database is the only authority). A client that cached
 * the session list would answer from memory across a rotation and be wrong in
 * exactly the situation that matters.
 */
export class Cma {
  readonly #config: ResolvedConfig;

  constructor(config: ClientConfig = {}) {
    this.#config = resolveConfig(config);
  }

  /** Where this client points and how it authenticates. */
  get config(): ResolvedConfig {
    return this.#config;
  }

  /** Sessions belonging to the authenticated subject, newest first. */
  listSessions(): Promise<readonly Session[]> {
    return sessions.listSessions(this.#config);
  }

  /** The runtimes this deployment offers, and what each can be. */
  listKernels(): Promise<readonly KernelOffer[]> {
    return sessions.listKernels(this.#config);
  }

  /** Provision a session. */
  createSession(options: CreateSession): Promise<Session> {
    return sessions.createSession(this.#config, options);
  }

  /** Move a session onto the other plane. A rotation; see `changePlane`. */
  changePlane(id: string, plane: SessionPlane): Promise<Session> {
    return sessions.changePlane(this.#config, id, plane);
  }

  /** End a session, leaving it listed and readable. */
  terminateSession(id: string): Promise<void> {
    return sessions.terminateSession(this.#config, id);
  }

  /** Release a session's sandbox, keeping the session. */
  stopSession(id: string): Promise<void> {
    return sessions.stopSession(this.#config, id);
  }

  /** Give a stopped session a sandbox again, under the same id. */
  reviveSession(id: string): Promise<Session> {
    return sessions.reviveSession(this.#config, id);
  }

  /** Remove a session and its history permanently. */
  deleteSession(id: string): Promise<void> {
    return sessions.deleteSession(this.#config, id);
  }

  /** Everything a session's terminal has shown, as raw bytes. */
  fetchTranscript(id: string): Promise<Uint8Array> {
    return sessions.fetchTranscript(this.#config, id);
  }
}

/**
 * Build a client.
 *
 * A function as well as a class because most consumers never need the type,
 * and `createClient({ baseUrl })` reads as configuration where `new Cma`
 * reads as construction.
 */
export function createClient(config: ClientConfig = {}): Cma {
  return new Cma(config);
}
