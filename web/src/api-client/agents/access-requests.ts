import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { array, enumeration, object, optional, text, type Output } from '../schema.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';

const services = (value: unknown) => {
  const parsed = array(text)(value);
  if (
    parsed.length === 0 ||
    parsed.length > 32 ||
    parsed.some((service) => !/^[A-Za-z0-9_-]{1,128}$/.test(service))
  )
    throw new ProtocolError('expected 1 to 32 bounded service identities');
  return parsed;
};
export const approveAgentAccessRequest = object({ services });
/** Exact service selection approved by the authenticated Agent owner. */
export type ApproveAgentAccessRequest = Output<typeof approveAgentAccessRequest>;
export const updateAgentServiceAccessRequest = object({ services, expected_services: services });
/** Replace shared Sandbox access only if the owner's observed selection still matches. */
export type UpdateAgentServiceAccessRequest = Output<typeof updateAgentServiceAccessRequest>;
const accessRequest = object({
  id: text,
  agent_id: text,
  status: enumeration('pending', 'connecting', 'applying', 'succeeded', 'denied', 'failed'),
  services,
  reason: text,
  retry_of: optional(text),
  connection: optional(
    object({ target: object({ type: enumeration('agent_access_request'), id: text }) }),
  ),
  error: optional(text),
});
/** Durable request; only succeeded proves that runtime access was verified. */
export type AgentAccessRequest = Output<typeof accessRequest>;
function projection(agentId: string, requestId?: string) {
  return (value: unknown): AgentAccessRequest => {
    const result = accessRequest(value);
    if (
      !result.id ||
      (result.retry_of !== undefined &&
        (!/^aar_[a-f0-9]{64}$/.test(result.retry_of) || result.retry_of === result.id)) ||
      result.agent_id !== agentId ||
      (requestId !== undefined && result.id !== requestId) ||
      (result.connection && result.connection.target.id !== result.id) ||
      (result.status === 'connecting' && !result.connection)
    )
      throw new ProtocolError('access request identity or connection did not match the request');
    return result;
  };
}
const path = (agentId: string) => `/api/v1/agents/${segment(agentId)}/access-requests`;
export function updateAgentServiceAccess(
  config: ResolvedConfig,
  agentId: string,
  body: UpdateAgentServiceAccessRequest,
  options: CommandOptions,
) {
  return read(config, `/api/v1/agents/${segment(agentId)}/service-access`, projection(agentId), {
    ...command(options),
    method: 'PUT',
    body: input(updateAgentServiceAccessRequest, body),
    expectedStatus: 202,
  });
}
export function listAgentAccessRequests(
  config: ResolvedConfig,
  agentId: string,
  options: OperationOptions,
) {
  return read(
    config,
    path(agentId),
    (value) => {
      const page = object({ items: array(projection(agentId)) })(value);
      if (new Set(page.items.map((item) => item.id)).size !== page.items.length)
        throw new ProtocolError('access requests contained repeated identities');
      return page;
    },
    options,
  );
}
export function approveAgentAccess(
  config: ResolvedConfig,
  agentId: string,
  requestId: string,
  body: ApproveAgentAccessRequest,
  options: CommandOptions,
) {
  return read(
    config,
    `${path(agentId)}/${segment(requestId)}/approve`,
    projection(agentId, requestId),
    {
      ...command(options),
      method: 'POST',
      body: input(approveAgentAccessRequest, body),
      expectedStatus: 200,
    },
  );
}
export function denyAgentAccess(
  config: ResolvedConfig,
  agentId: string,
  requestId: string,
  options: CommandOptions,
) {
  return read(
    config,
    `${path(agentId)}/${segment(requestId)}/deny`,
    projection(agentId, requestId),
    {
      ...command(options),
      method: 'POST',
      body: {},
      expectedStatus: 200,
    },
  );
}
