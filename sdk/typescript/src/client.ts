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
import * as discovery from './discovery.js';
import * as environment from './environment.js';
import * as events from './events.js';
import * as operations from './operations.js';
import * as resources from './resources.js';
import * as sessions from './sessions.js';
import * as settings from './settings.js';
import * as sponsorships from './sponsorships.js';
import * as triggers from './triggers.js';
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
import type { Conformance } from './discovery.js';
import type { ChatSettings, StoredChatSettings } from './settings.js';
import type {
  SponsoredGrant,
  SponsorshipCredential,
  SponsorshipPageRequest,
  SponsorshipPolicy,
} from './sponsorships.js';

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

  /** Public protocol and build discovery; no configured bearer is presented. */
  readConformance(options: OperationOptions = {}): Promise<Conformance> {
    return discovery.readConformance(this.#config, options);
  }

  /** Read the deployment-owned chat setting and its write revision. */
  readChatSettings(options: OperationOptions = {}): Promise<StoredChatSettings> {
    return settings.readChatSettings(this.#config, options);
  }

  /** Replace only the chat setting under the platform revision CAS. */
  changeChatSettings(
    expectedRevision: number,
    chat: ChatSettings | null,
    options: OperationOptions = {},
  ): Promise<StoredChatSettings> {
    return settings.changeChatSettings(this.#config, expectedRevision, chat, options);
  }

  /** Read one public, side-effect-free README Trigger preview. */
  readTriggerPreview(
    publicId: string,
    options: OperationOptions = {},
  ): Promise<triggers.TriggerPreview> {
    return triggers.readTriggerPreview(this.#config, publicId, options);
  }

  /** Store a publisher Trigger draft. */
  createTrigger(
    selectedServiceId: string,
    content: triggers.TriggerContentDraft,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.TriggerDraft> {
    return triggers.createTrigger(this.#config, selectedServiceId, content, options);
  }

  /** List one cursor page of publisher Trigger drafts. */
  listTriggers(
    limit = 50,
    cursor?: string,
    options: OperationOptions = {},
  ): Promise<triggers.CursorPage<triggers.TriggerDraft>> {
    return triggers.listTriggers(this.#config, limit, cursor, options);
  }

  /** Read one publisher Trigger draft. */
  readTrigger(id: string, options: OperationOptions = {}): Promise<triggers.TriggerDraft> {
    return triggers.readTrigger(this.#config, id, options);
  }

  /** Replace one publisher Trigger draft under optimistic concurrency. */
  updateTrigger(
    id: string,
    expectedVersion: number,
    selectedServiceId: string,
    content: triggers.TriggerContentDraft,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.TriggerDraft> {
    return triggers.updateTrigger(
      this.#config,
      id,
      expectedVersion,
      selectedServiceId,
      content,
      options,
    );
  }

  /** Publish one immutable Trigger revision. */
  publishTrigger(
    id: string,
    expectedVersion: number,
    selectedServiceId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.TriggerRevision> {
    return triggers.publishTrigger(this.#config, id, expectedVersion, selectedServiceId, options);
  }

  /** List one cursor page of immutable Trigger revisions. */
  listTriggerRevisions(
    limit = 50,
    cursor?: string,
    options: OperationOptions = {},
  ): Promise<triggers.CursorPage<triggers.TriggerRevision>> {
    return triggers.listTriggerRevisions(this.#config, limit, cursor, options);
  }

  /** Read one immutable publisher Trigger revision. */
  readTriggerRevision(
    id: string,
    options: OperationOptions = {},
  ): Promise<triggers.TriggerRevision> {
    return triggers.readTriggerRevision(this.#config, id, options);
  }

  /** Retire a revision from new Launch creation without changing old Launches. */
  retireTriggerRevision(
    id: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.TriggerRevision> {
    return triggers.retireTriggerRevision(this.#config, id, options);
  }

  /** List reusable NyxID GitHub connections for the current subject. */
  listTriggerConnections(
    options: OperationOptions = {},
  ): Promise<readonly triggers.TriggerConnection[]> {
    return triggers.listTriggerConnections(this.#config, options);
  }

  /** Begin explicit GitHub consent in NyxID. */
  beginTriggerConnection(
    intentId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.ConnectionAuthorization> {
    return triggers.beginTriggerConnection(this.#config, intentId, options);
  }

  /** Reconnect one exact reusable NyxID GitHub service. */
  reconnectTriggerConnection(
    serviceId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.ConnectionAuthorization> {
    return triggers.reconnectTriggerConnection(this.#config, serviceId, options);
  }

  /** Revoke one exact reusable NyxID GitHub service. */
  revokeTriggerConnection(
    serviceId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.TriggerConnection> {
    return triggers.revokeTriggerConnection(this.#config, serviceId, options);
  }

  /** Revalidate one exact reusable GitHub connection. */
  preflightTriggerConnection(
    serviceId: string,
    options: OperationOptions = {},
  ): Promise<triggers.TriggerConnection> {
    return triggers.preflightTriggerConnection(this.#config, serviceId, options);
  }

  /** Resolve a pasted repository owner/name through one exact GitHub connection. */
  resolveTriggerRepository(
    serviceId: string,
    owner: string,
    name: string,
    options: OperationOptions = {},
  ): Promise<triggers.TriggerRepositorySelection> {
    return triggers.resolveTriggerRepository(this.#config, serviceId, owner, name, options);
  }

  /** Create or resume this subject's current Launch for one revision. */
  createOrResumeLaunch(
    publicId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.createOrResumeLaunch(this.#config, publicId, options);
  }

  /** Create a separate Launch after explicitly accepting extra resources. */
  createNewLaunch(
    publicId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.createNewLaunch(this.#config, publicId, options);
  }

  /** List one bounded Launch cursor page. */
  listLaunches(
    page: triggers.LaunchListRequest = {},
    options: OperationOptions = {},
  ): Promise<triggers.CursorPage<triggers.Launch>> {
    return triggers.listLaunches(this.#config, page, options);
  }

  /** Read one owned Launch. */
  readLaunch(id: string, options: OperationOptions = {}): Promise<triggers.Launch> {
    return triggers.readLaunch(this.#config, id, options);
  }

  /** Replace the editable fields on one Launch review. */
  updateLaunchOverrides(
    id: string,
    expectedRevision: number,
    overrides: triggers.LaunchOverrides,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.updateLaunchOverrides(this.#config, id, expectedRevision, overrides, options);
  }

  /** Select one exact reusable GitHub connection for a Launch. */
  selectLaunchConnection(
    id: string,
    expectedRevision: number,
    serviceId: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.selectLaunchConnection(this.#config, id, expectedRevision, serviceId, options);
  }

  /** Explicitly Start the exact reviewed Launch snapshot. */
  startLaunch(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    acknowledgeWorkflowRisk: boolean,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.startLaunch(
      this.#config,
      id,
      expectedRevision,
      snapshotDigest,
      acknowledgeWorkflowRisk,
      options,
    );
  }

  /** Authorize one replacement initial-prompt dispatch after an ambiguous write. */
  recoverInitialPrompt(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.recoverInitialPrompt(
      this.#config,
      id,
      expectedRevision,
      snapshotDigest,
      options,
    );
  }

  /** Request a sealed diff checkpoint without publishing it. */
  requestLaunchDiff(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.requestLaunchDiff(this.#config, id, expectedRevision, snapshotDigest, options);
  }

  /** Read the sealed diff and current contribution/PR status. */
  readLaunchDiff(id: string, options: OperationOptions = {}): Promise<triggers.DiffReview> {
    return triggers.readLaunchDiff(this.#config, id, options);
  }

  /** Explicitly publish the exact reviewed diff to the Launch branch. */
  publishLaunchBranch(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.publishLaunchBranch(
      this.#config,
      id,
      expectedRevision,
      snapshotDigest,
      options,
    );
  }

  /** Seal the exact Open or Update pull-request proposal without writing GitHub. */
  reviewPullRequest(
    id: string,
    operation: 'open' | 'update',
    edits: { readonly title?: string; readonly body?: string } = {},
    options: OperationOptions = {},
  ): Promise<triggers.PullRequestDraft> {
    return triggers.reviewPullRequest(this.#config, id, operation, edits, options);
  }

  /** Explicitly confirm the exact sealed Open or Update proposal. */
  confirmPullRequest(
    id: string,
    operation: 'open' | 'update',
    expectedRevision: number,
    snapshotDigest: string,
    edits: { readonly title?: string; readonly body?: string } = {},
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.confirmPullRequest(
      this.#config,
      id,
      operation,
      expectedRevision,
      snapshotDigest,
      edits,
      options,
    );
  }

  /** Queue an immediate read-only provider status observation for an open pull request. */
  refreshPullRequestStatus(
    id: string,
    expectedRevision: number,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.refreshPullRequestStatus(this.#config, id, expectedRevision, options);
  }

  /** Cancel one Launch while retaining its durable history. */
  cancelLaunch(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.cancelLaunch(this.#config, id, expectedRevision, snapshotDigest, options);
  }

  completeLaunch(
    id: string,
    expectedRevision: number,
    snapshotDigest: string,
    options: triggers.TriggerCommandOptions,
  ): Promise<triggers.Launch> {
    return triggers.completeLaunch(this.#config, id, expectedRevision, snapshotDigest, options);
  }

  /** Replay durable Launch progress after one accepted cursor. */
  readLaunchProgress(
    id: string,
    after = 0,
    limit = 100,
    options: OperationOptions = {},
  ): Promise<triggers.LaunchProgress> {
    return triggers.readLaunchProgress(this.#config, id, after, limit, options);
  }

  /** Download one immutable complete diff artifact. */
  downloadDiffArtifact(
    launchId: string,
    artifactId: string,
    options: OperationOptions = {},
  ): Promise<Uint8Array> {
    return triggers.downloadDiffArtifact(this.#config, launchId, artifactId, options);
  }

  /** One bounded page of secret-free credentials controlled by this owner. */
  listSponsorshipCredentialsPage(
    page: SponsorshipPageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<SponsorshipCredential>> {
    return sponsorships.listSponsorshipCredentialsPage(this.#config, page, options);
  }

  /** One bounded page of policies and accounting controlled by this owner. */
  listSponsorshipPoliciesPage(
    page: SponsorshipPageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<SponsorshipPolicy>> {
    return sponsorships.listSponsorshipPoliciesPage(this.#config, page, options);
  }

  /** One bounded page of grants attributed to this contributor. */
  listContributorSponsorshipGrantsPage(
    page: SponsorshipPageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<SponsoredGrant>> {
    return sponsorships.listContributorSponsorshipGrantsPage(this.#config, page, options);
  }

  /** One bounded page of grants controlled by this sponsor. */
  listSponsorSponsorshipGrantsPage(
    page: SponsorshipPageRequest = {},
    options: OperationOptions = {},
  ): Promise<Page<SponsoredGrant>> {
    return sponsorships.listSponsorSponsorshipGrantsPage(this.#config, page, options);
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

  /** Set the preference and receive the complete post-write profile collection. */
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
