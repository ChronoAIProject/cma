/** Typed, repository-only client for CMA's public resource APIs. */

export { Cma, createClient } from './client.js';
export { Agents, type AgentListOptions } from './agents.js';
export { Sandboxes, type SandboxListOptions } from './sandboxes.js';
export { Workspaces, Worktrees } from './workspaces.js';
export { Profiles } from './profiles.js';
export { Marketplace, type MarketplaceListOptions } from './marketplace.js';
export type { CommandOptions, EventOptions, PageOptions, ResourcePage } from './resources.js';
export type * from './agents/types.js';
export type * from './agents/events.js';
export type * from './instances/types.js';
export type * from './instances/events.js';
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
export { readConformance, toConformance, type Conformance } from './discovery.js';
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
