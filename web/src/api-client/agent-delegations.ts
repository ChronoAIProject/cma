import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { input, read } from './resources.js';
import { nullable, object, optional, positiveInteger, text, type Output } from './schema.js';
import { resourceOwner } from './resource-access.js';

const jsonValue = (value: unknown) => value;
const identityRequest = object({ event_ref: text });
const identity = object({
  grant_id: nullable(text),
  subject: nullable(text),
  display_name: nullable(text),
  login_url: nullable(text),
});
const operationRequest = object({
  event_ref: text,
  owner: optional(nullable(resourceOwner)),
  method: text,
  path: text,
  body: optional(jsonValue),
  idempotency_key: optional(nullable(text)),
});
const operationResult = object({ status: positiveInteger, body: jsonValue });

export type AgentDelegationIdentityRequest = Output<typeof identityRequest>;
export type AgentDelegationIdentity = Output<typeof identity>;
export type AgentDelegationOperationRequest = Output<typeof operationRequest>;
export type AgentDelegationOperationResult = Output<typeof operationResult>;

/** Provider operations bound to the current event and the user's CMA consent. */
export class AgentDelegations {
  constructor(private readonly config: ResolvedConfig) {}

  /** Resolve user sign-in status; the returned URL requires explicit browser consent. */
  identity(body: AgentDelegationIdentityRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/agent-delegations/identity', identity, {
      ...options,
      method: 'POST',
      body: input(identityRequest, body),
    });
  }

  /** Dispatch a permitted local CMA operation. Inspect the inner HTTP status. */
  operate(body: AgentDelegationOperationRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/agent-delegations/operations', operationResult, {
      ...options,
      method: 'POST',
      body: input(operationRequest, body),
    });
  }
}
