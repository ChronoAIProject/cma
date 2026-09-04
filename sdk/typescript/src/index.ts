/**
 * Temporary CMA discovery client.
 *
 * Resource APIs return with the replacement public contract in issue #819.
 * This package intentionally exposes only the live anonymous discovery route
 * and transport primitives that do not encode a retired domain model.
 */

export { Cma, createClient } from './client.js';
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
