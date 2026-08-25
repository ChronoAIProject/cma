/**
 * The CMA client SDK.
 *
 * One import for everything a consumer needs to drive a CMA deployment:
 * start a session, read what it may do, hold a conversation with it, and be
 * told in words when it refuses.
 *
 * ## Why this exists
 *
 * `CLAUDE.md` §1 states what CMA is for: its clients are other
 * products' frontends and backends, and this repository's own `web/` is the
 * reference client — held to the same constraints a third party is, because a
 * client permitted one private route would prove nothing about the one
 * somebody else has to build. This package is the other half of that. The
 * dashboard consumes exactly what is exported here, so an interface it needs
 * and this does not have is a gap that shows up as a build failure rather than
 * as a discovery six months later.
 *
 * ## Getting started
 *
 * ```ts
 * import { createClient } from '@chronoai/cma-sdk';
 *
 * const bot = createClient({
 *   baseUrl: 'https://bot-api.example',
 *   credential: () => tokenStore.current(),
 *   cookies: false, // cross-origin: do not send the browser's own
 * });
 *
 * const session = await bot.createSession({ cols: 120, rows: 32 });
 * if (!session.capabilities.conversation) {
 *   throw new Error('this deployment did not grant this session a conversation');
 * }
 * ```
 *
 * The dashboard's own configuration is the empty one — same-origin paths and
 * the cookie the browser attaches — which is why every default is that.
 *
 */

export { Cma, createClient } from './client.js';
export {
  resolveConfig,
  type ClientConfig,
  type Credential,
  type RequestIdProvider,
  type ResolvedConfig,
  type ResponseMetadata,
  type ResponseObserver,
} from './config.js';
export { readConformance, toConformance, type Conformance } from './discovery.js';
export {
  ApiError,
  CapabilityRefused,
  CmaError,
  CredentialError,
  DeploymentAtCapacity,
  ProtocolError,
  RequestAborted,
  RequestIdError,
  ResponseObserverError,
  SessionLimitReached,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
export {
  errorFromResponse,
  pageTarget,
  request,
  requestBytes,
  requestPage,
  requestPages,
  requestResponse,
  requestVoid,
  MAX_PAGE_SIZE,
  MIN_PAGE_SIZE,
  type OperationOptions,
  type Page,
  type PageContinuation,
  type PageRequest,
  type PageWalkOptions,
  type RequestOptions,
} from './http.js';
export {
  createEnvironmentProfile,
  deleteEnvironmentProfile,
  listEnvironmentProfiles,
  listEnvironmentProfilesPage,
  preferEnvironmentProfile,
  readEnvironmentProfile,
  readServiceCatalogue,
  readSkillCatalogue,
  toEnvironmentProfile,
  updateEnvironmentProfile,
  type CatalogSkill,
  type EnvironmentAgent,
  type EnvironmentAgentDraft,
  type EnvironmentProfile,
  type EnvironmentProfileDraft,
  type EnvironmentProfilePageRequest,
  type MandatoryEntries,
  type NyxIdAccess,
  type NyxIdAccessDraft,
  type PlatformService,
  type ProfilePackage,
  type ProfileSessionSize,
  type ProfileVariable,
  type ServiceCatalogue,
  type SkillCatalogue,
} from './environment.js';
export {
  KNOWN_SESSION_EVENT_TYPES,
  eventFromMessage,
  streamEvents,
  type KnownSessionEventType,
  type SessionEvent,
  type SessionEventKind,
  type StreamEventsOptions,
  type UnknownSessionEventKind,
} from './events.js';
export {
  getUsage,
  interrupt,
  resizeTerminal,
  resolveApproval,
  screenSnapshot,
  sendInput,
  steer,
  type ApprovalDecision,
  type ScreenSnapshot,
  type SessionInput,
  type SessionUsage,
  type TerminalGeometry,
} from './operations.js';
export {
  readResources,
  toResourceStatus,
  type ResourceStatusReason,
  type SessionResourceStatus,
} from './resources.js';
export {
  changePlane,
  createSession,
  deleteSession,
  fetchTranscript,
  listKernels,
  listSessionPage,
  listSessions,
  quoteSession,
  readAllowance,
  readSpend,
  readSession,
  reviveSession,
  stopSession,
  terminateSession,
  toSession,
  STOP_REASONS,
  type CreateSession,
  type PricedQuote,
  type PricedSpend,
  type PricingQuote,
  type SessionAllowance,
  type SessionListStatus,
  type SessionPageRequest,
  type SessionSpend,
  type UnpricedQuote,
  type UnpricedSpend,
} from './sessions.js';
export type {
  CredentialMode,
  Kernel,
  KernelOffer,
  PermissionMode,
  ReasoningEffort,
  Session,
  SessionAgent,
  SessionCreator,
  SessionLifecycle,
  SessionPlane,
  SessionSize,
  SessionService,
  SessionSkill,
  RecoveryState,
  StopReason,
} from './types.js';
export {
  GAP_VALUE_FRAMES,
  KNOWN_EVENTS,
  LIVE_MARKER,
  PATHS,
  PRICING_WIRE,
  STATE_DELTA_PATCHES,
  frameKey,
  initialState,
  type AguiState,
} from './wire.js';

/*
 * The conversation half, listed explicitly rather than star-exported so this file states the SDK's surface
 * in one place — a consumer reading it should not have to open two more files
 * to learn what they may call.
 */
export {
  AguiConnection,
  AguiOperations,
  NO_CAPABILITIES,
  applyStateDelta,
  createAguiTransport,
  createOperationTransport,
  fetchTransport,
  parseEventStream,
  toCapabilities,
  toEvent,
  wireTransport,
  type AguiEvent,
  type AguiTransport,
  type ConcurrentInput,
  type ConnectionOptions,
  type Interrupt,
  type InterruptOption,
  type JsonPatch,
  type LinkFailure,
  type LinkStatus,
  type OperationTransport,
  type SessionCapabilities,
  type StateDelta,
} from './agui/index.js';
export {
  EMPTY,
  fold,
  initialConversation,
  reduce,
  type Conversation,
  type AvailableCommand,
  type ConfigOption,
  type ContextCompactionEntry,
  type Entry,
  type FailureEntry,
  type GapEntry,
  type MessageEntry,
  type MemoryPressure,
  type PendingApproval,
  type PlanStep,
  type Resources,
  type RuntimeLoss,
  type SignIn,
  type ToolBody,
  type ToolCallEntry,
  type UnknownEntry,
  type Usage,
} from './conversation/index.js';
