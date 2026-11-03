/** Typed, repository-only client for CMA's public resource APIs. */

export { Cma, createClient } from './client.js';
export * from './resource-access.js';
export * from './agent-files.js';
export * from './bots.js';
export * from './bot-setups.js';
export * from './agent-delegations.js';
export * from './gateway-interactions.js';
export {
  Agents,
  type AgentListOptions,
  type AccessibleAgentListOptions,
  type AgentProvenanceInventoryOptions,
} from './agents.js';
export {
  Sandboxes,
  type SandboxListOptions,
  type AccessibleSandboxListOptions,
} from './sandboxes.js';
export { Workspaces, Worktrees } from './workspaces.js';
export { Profiles } from './profiles.js';
export { Marketplace, type MarketplaceListOptions } from './marketplace.js';
export { readDefaults, type PlatformDefaults } from './defaults.js';
export * from './admin/index.js';
export * from './profile-authoring/index.js';
export * from './triggers.js';
export * from './auth.js';
export * from './responses.js';
export * from './responses-v2.js';
export * from './agent-events-v2/index.js';
export * from './agui/index.js';
export * from './launch/index.js';
export type { CommandOptions, EventOptions, PageOptions, ResourcePage } from './resources.js';
export type * from './agents/types.js';
export type * from './agents/lifecycle.js';
export type * from './agents/models.js';
export type * from './agents/skills.js';
export type * from './agents/access-requests.js';
export type * from './agents/shared.js';
export type * from './agents/provenance.js';
export type * from './agents/artifacts.js';
export type * from './agents/deliverables.js';
export { DeliverableUnavailable } from './agents/deliverables.js';
export type * from './agents/attachments.js';
export type * from './agents/channel-usage.js';
export type * from './instances/shared.js';
export type * from './agents/events.js';
export type * from './instances/types.js';
export type * from './instances/events.js';
export type * from './instances/commands.js';
export type * from './profiles/types.js';
export type * from './profiles/marketplace.js';
export {
  headersFor,
  observeResponse,
  resolveConfig,
  resolveRequestId,
  responseRequestId,
  urlOf,
  type ClientConfig,
  type Credential,
  type RequestIdProvider,
  type ResolvedConfig,
  type ResponseMetadata,
  type ResponseObserver,
} from './config.js';
export {
  readConformance,
  readOpenApi,
  toConformance,
  type Conformance,
  type OpenApiDocument,
} from './discovery.js';
export { UnsupportedInterfaces } from './unsupported.js';
export {
  ApiError,
  CmaError,
  CredentialError,
  ProtocolError,
  RequestAborted,
  RequestIdError,
  ResponseObserverError,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
export {
  errorFromResponse,
  raceWithAbortSignal,
  request,
  requestBytes,
  requestResponse,
  requestVoid,
  throwIfSignalAborted,
  withDeadline,
  withOperationDeadline,
  type OperationOptions,
  type RequestOptions,
} from './http.js';

export { readAgentCard, type ProviderAgentCard } from './agent-card.js';

export * from './gateway-records.js';

export * from './provider-bindings.js';

export { Cmaeg, type CmaegConfig } from './cmaeg/index.js';
export * as cmaeg from './cmaeg/index.js';

export * from './event-worker-profiles.js';

export type { AgentSettings, UpdateAgentSettingsRequest } from './agents/settings.js';

export type {
  MessageVote,
  MessageFeedback,
  MessageFeedbackPage,
} from './agents/message-feedback.js';

export * from './documents.js';

export type { AgentPreferences, UpdateAgentPreferencesRequest } from './agents/preferences.js';
