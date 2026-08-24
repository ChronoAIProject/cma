/**
 * One client, configured once.
 *
 * The functions underneath all take a config as their first argument, which is
 * what makes two clients in one process possible. This is the shape a consumer
 * actually wants to hold: `cma.listSessions()` rather than
 * `listSessions(config)` threaded through every call site.
 *
 * Both are exported. A React app holds the object; a test that wants one call
 * against a throwaway config uses the function directly and constructs nothing.
 */

import { resolveConfig, type ClientConfig, type ResolvedConfig } from './config.js';
import * as environment from './environment.js';
import * as events from './events.js';
import * as operations from './operations.js';
import * as resources from './resources.js';
import * as sessions from './sessions.js';
import type { SessionResourceStatus } from './resources.js';
import type {
  CreateSession,
  PricingQuote,
  SessionAllowance,
  SessionPageRequest,
  SessionSpend,
} from './sessions.js';
import type { OperationOptions, Page, PageWalkOptions } from './http.js';
import type {
  EnvironmentProfile,
  EnvironmentProfileDraft,
  EnvironmentProfilePageRequest,
  ServiceCatalogue,
  SkillCatalogue,
} from './environment.js';
import type { StreamEventsOptions, SessionEvent } from './events.js';
import type {
  ApprovalDecision,
  ScreenSnapshot,
  SessionInput,
  SessionUsage,
  TerminalGeometry,
} from './operations.js';
import type { KernelOffer, Session, SessionPlane } from './types.js';

/**
 * A configured CMA client.
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

  /** Every environment profile owned by the authenticated subject. */
  listEnvironmentProfiles(options: PageWalkOptions = {}): Promise<readonly EnvironmentProfile[]> {
    return environment.listEnvironmentProfiles(this.#config, options);
  }

  /** One bounded profile page, with an opaque continuation when more exist. */
  listEnvironmentProfilesPage(
    page: EnvironmentProfilePageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<EnvironmentProfile>> {
    return environment.listEnvironmentProfilesPage(this.#config, page, options);
  }

  /** Read one owned environment profile. */
  readEnvironmentProfile(id: string, options: OperationOptions = {}): Promise<EnvironmentProfile> {
    return environment.readEnvironmentProfile(this.#config, id, options);
  }

  /** Store an environment profile. */
  createEnvironmentProfile(
    draft: EnvironmentProfileDraft,
    options: OperationOptions = {},
  ): Promise<EnvironmentProfile> {
    return environment.createEnvironmentProfile(this.#config, draft, options);
  }

  /** Replace every caller-controlled field on an environment profile. */
  updateEnvironmentProfile(
    id: string,
    draft: EnvironmentProfileDraft,
    options: OperationOptions = {},
  ): Promise<EnvironmentProfile> {
    return environment.updateEnvironmentProfile(this.#config, id, draft, options);
  }

  /** Delete one environment profile. */
  deleteEnvironmentProfile(id: string, options: OperationOptions = {}): Promise<void> {
    return environment.deleteEnvironmentProfile(this.#config, id, options);
  }

  /** Set the preferred profile, or clear the preference with `null`. */
  preferEnvironmentProfile(
    id: string | null,
    options: OperationOptions = {},
  ): Promise<readonly EnvironmentProfile[]> {
    return environment.preferEnvironmentProfile(this.#config, id, options);
  }

  /** Skills available while authoring an environment profile. */
  readSkillCatalogue(options: OperationOptions = {}): Promise<SkillCatalogue> {
    return environment.readSkillCatalogue(this.#config, options);
  }

  /** NyxID services available while authoring an environment profile. */
  readServiceCatalogue(options: OperationOptions = {}): Promise<ServiceCatalogue> {
    return environment.readServiceCatalogue(this.#config, options);
  }

  /** Sessions belonging to the authenticated subject, newest first. */
  listSessions(options: PageWalkOptions = {}): Promise<readonly Session[]> {
    return sessions.listSessions(this.#config, options);
  }

  /** One bounded session page, optionally narrowed to one exact lifecycle. */
  listSessionPage(
    page: SessionPageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<Session>> {
    return sessions.listSessionPage(this.#config, page, options);
  }

  /** The authenticated subject's advisory live-session allowance. */
  readAllowance(options: OperationOptions = {}): Promise<SessionAllowance> {
    return sessions.readAllowance(this.#config, options);
  }

  /**
   * Read one session as the control plane currently describes it.
   *
   * A session's record is not complete when it is created: some of what a
   * client draws is settled during provisioning, and a client holding the
   * object `createSession` returned is holding the answer from before that.
   */
  readSession(id: string, options: OperationOptions = {}): Promise<Session> {
    return sessions.readSession(this.#config, id, options);
  }

  /** The runtimes this deployment offers, and what each can be. */
  listKernels(options: OperationOptions = {}): Promise<readonly KernelOffer[]> {
    return sessions.listKernels(this.#config, options);
  }

  /** Provision a session. */
  createSession(session: CreateSession = {}, options: OperationOptions = {}): Promise<Session> {
    return sessions.createSession(this.#config, session, options);
  }

  /** Quote the exact hourly rate a new session would pin. */
  quoteSession(
    environmentProfile?: string | null,
    options: OperationOptions = {},
  ): Promise<PricingQuote> {
    return sessions.quoteSession(this.#config, environmentProfile, options);
  }

  /** Read one session's reserved-capacity spend. */
  readSpend(id: string, options: OperationOptions = {}): Promise<SessionSpend> {
    return sessions.readSpend(this.#config, id, options);
  }

  /** Read one session's latest sandbox-resource status. */
  readResources(id: string, options: OperationOptions = {}): Promise<SessionResourceStatus> {
    return resources.readResources(this.#config, id, options);
  }

  /** Move a session onto the other plane. A rotation; see `changePlane`. */
  changePlane(id: string, plane: SessionPlane, options: OperationOptions = {}): Promise<Session> {
    return sessions.changePlane(this.#config, id, plane, options);
  }

  /** End a session, leaving it listed and readable. */
  terminateSession(id: string, options: OperationOptions = {}): Promise<void> {
    return sessions.terminateSession(this.#config, id, options);
  }

  /** Release a session's sandbox, keeping the session. */
  stopSession(id: string, options: OperationOptions = {}): Promise<void> {
    return sessions.stopSession(this.#config, id, options);
  }

  /** Give a stopped session a sandbox again, under the same id. */
  reviveSession(id: string, options: OperationOptions = {}): Promise<Session> {
    return sessions.reviveSession(this.#config, id, options);
  }

  /** Remove a session and its history permanently. */
  deleteSession(id: string, options: OperationOptions = {}): Promise<void> {
    return sessions.deleteSession(this.#config, id, options);
  }

  /** Everything a session's terminal has shown, as raw bytes. */
  fetchTranscript(id: string, options: OperationOptions = {}): Promise<Uint8Array> {
    return sessions.fetchTranscript(this.#config, id, options);
  }

  /** Deliver text or terminal bytes on the session's granted plane. */
  sendInput(id: string, input: SessionInput, options: OperationOptions = {}): Promise<void> {
    return operations.sendInput(this.#config, id, input, options);
  }

  /** Open a reconnecting core event stream after an optional cursor. */
  streamEvents(id: string, options: StreamEventsOptions = {}): AsyncIterable<SessionEvent> {
    return events.streamEvents(this.#config, id, options);
  }

  /** Resolve one pending approval. */
  resolveApproval(
    id: string,
    requestId: string,
    decision: ApprovalDecision,
    options: OperationOptions = {},
  ): Promise<void> {
    return operations.resolveApproval(this.#config, id, requestId, decision, options);
  }

  /** Stop a turn in flight without ending the session. */
  interrupt(id: string, options: OperationOptions = {}): Promise<void> {
    return operations.interrupt(this.#config, id, options);
  }

  /** Redirect a conversation turn already in flight. */
  steer(id: string, text: string, options: OperationOptions = {}): Promise<void> {
    return operations.steer(this.#config, id, text, options);
  }

  /** Inform a terminal session of its new geometry. */
  resizeTerminal(
    id: string,
    geometry: TerminalGeometry,
    options: OperationOptions = {},
  ): Promise<void> {
    return operations.resizeTerminal(this.#config, id, geometry, options);
  }

  /** Read the current rendered terminal screen. */
  screenSnapshot(id: string, options: OperationOptions = {}): Promise<ScreenSnapshot> {
    return operations.screenSnapshot(this.#config, id, options);
  }

  /** Read cumulative token and context usage. */
  getUsage(id: string, options: OperationOptions = {}): Promise<SessionUsage> {
    return operations.getUsage(this.#config, id, options);
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
