import type { Dispatch } from './context.js';

export const resourceDispatch: Record<string, Dispatch> = {
  'GET /api/v1/resource-access/{kind}/{id}': ({ client, param }) =>
    client.resourceAccess.effective(param('kind'), param('id')),
  'GET /api/v1/resource-shares/{kind}/{id}': ({ client, param }) =>
    client.resourceAccess.listShares(param('kind'), param('id')),
  'GET /api/v1/resource-shares/{kind}/{id}/{share_id}': ({ client, param }) =>
    client.resourceAccess.readShare(param('kind'), param('id'), param('share_id')),
  'POST /api/v1/resource-shares/{kind}/{id}': ({ client, param, body, command }) =>
    client.resourceAccess.createShare(param('kind'), param('id'), body, command),
  'PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}': ({ client, param, body, command }) =>
    client.resourceAccess.updateShare(param('kind'), param('id'), param('share_id'), body, command),
  'DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}': ({ client, param, query, command }) =>
    client.resourceAccess.revokeShare(
      param('kind'),
      param('id'),
      param('share_id'),
      Number(query.get('expected_revision')),
      command,
    ),
  'GET /api/v1/conformance': ({ client }) => client.readConformance(),
  'GET /openapi.json': ({ client }) => client.readOpenApi(),
  'GET /api/v1/defaults': ({ client }) => client.readDefaults(),
  'GET /api/v1/sandboxes': ({ client, options }) => client.sandboxes.list(options),
  'POST /api/v1/sandboxes': ({ client, body, command }) => client.sandboxes.create(body, command),
  'GET /api/v1/sandboxes/{id}': ({ client, param }) => client.sandboxes.read(param('id')),
  'DELETE /api/v1/sandboxes/{id}': ({ client, param, command }) =>
    client.sandboxes.delete(param('id'), command),
  'POST /api/v1/sandboxes/{id}/pause': ({ client, param, command }) =>
    client.sandboxes.pause(param('id'), command),
  'POST /api/v1/sandboxes/{id}/revive': ({ client, param, command }) =>
    client.sandboxes.revive(param('id'), command),
  'POST /api/v1/sandboxes/{id}/service-access': ({ client, param, body, command }) =>
    client.sandboxes.grantServiceAccess(param('id'), body, command),
  'GET /api/v1/sandboxes/{id}/events': ({ client, param, options }) =>
    client.sandboxes.events(param('id'), options),
  'GET /api/v1/sandboxes/{id}/workspaces': ({ client, param, options }) =>
    client.workspaces.list(param('id'), options),
  'POST /api/v1/sandboxes/{id}/workspaces': ({ client, param, body, command }) =>
    client.workspaces.create(param('id'), body, command),
  'GET /api/v1/workspaces/{id}': ({ client, param }) => client.workspaces.read(param('id')),
  'DELETE /api/v1/workspaces/{id}': ({ client, param, command }) =>
    client.workspaces.delete(param('id'), command),
  'GET /api/v1/workspaces/{id}/worktrees': ({ client, param, options }) =>
    client.worktrees.list(param('id'), options),
  'GET /api/v1/worktrees/{id}': ({ client, param }) => client.worktrees.read(param('id')),
  'GET /api/v1/agents': ({ client, options }) => client.agents.list(options),
  'GET /api/v1/agents/{id}': ({ client, param }) => client.agents.readAccessible(param('id')),
  'GET /api/v1/agents/{id}/source': ({ client, param }) => client.agents.source(param('id')),
  'POST /api/v1/workspaces/{id}/agents': ({ client, param, body, command }) =>
    client.agents.create(param('id'), body, command),
  'DELETE /api/v1/agents/{id}': ({ client, param, command }) =>
    client.agents.delete(param('id'), command),
  'POST /api/v1/agents/{id}/input': ({ client, param, body, command }) =>
    client.agents.input(param('id'), body, command),
  'POST /api/v1/agents/{id}/steer': ({ client, param, body, command }) =>
    client.agents.steer(param('id'), body, command),
  'POST /api/v1/agents/{id}/interrupt': ({ client, param, command }) =>
    client.agents.interrupt(param('id'), command),
  'POST /api/v1/agents/{id}/stop': ({ client, param, body, command }) =>
    client.agents.stop(param('id'), body, command),
  'POST /api/v1/agents/{id}/approvals/{request_id}': ({ client, param, body, command }) =>
    client.agents.answerApproval(param('id'), param('request_id'), body, command),
  'GET /api/v1/agents/{id}/events': ({ client, param, options }) =>
    client.agents.events(param('id'), options),
  'GET /api/v1/agents/{id}/transcript': ({ client, param }) =>
    client.agents.transcript(param('id')),
  'GET /api/v1/agents/{id}/usage': ({ client, param }) => client.agents.usage(param('id')),
  'GET /api/v1/agents/{id}/resources': ({ client, param }) => client.agents.resources(param('id')),
  'GET /api/v1/agents/{id}/terminal': ({ client, param }) => client.agents.terminal(param('id')),
};

for (const kind of ['agent', 'workspace', 'sandbox'] as const) {
  const base = `/api/v1/${kind}-profiles`;
  const resource = `${kind}Profiles` as const;
  resourceDispatch[`GET ${base}`] = ({ client, options }) => client[resource].list(options);
  resourceDispatch[`POST ${base}`] = ({ client, body, command }) =>
    client[resource].create(body, command);
  resourceDispatch[`GET ${base}/{id}`] = ({ client, param }) => client[resource].read(param('id'));
  resourceDispatch[`PUT ${base}/{id}`] = ({ client, param, body, command }) =>
    client[resource].save(param('id'), body, command);
  resourceDispatch[`POST ${base}/{id}/revisions`] = ({ client, param, body, command }) =>
    client[resource].publish(param('id'), body, command);
  resourceDispatch[`GET ${base}/{id}/revisions`] = ({ client, param, options }) =>
    client[resource].listRevisions(param('id'), options);
  resourceDispatch[`GET ${base}/{id}/revisions/{revision}`] = ({ client, param }) =>
    client[resource].readRevision(param('id'), Number(param('revision')));
  resourceDispatch[`PUT ${base}/{id}/visibility`] = ({ client, param, body, command }) =>
    client[resource].setVisibility(param('id'), body, command);
  resourceDispatch[`POST ${base}/{id}/retire`] = ({ client, param, command }) =>
    client[resource].retire(param('id'), command);
}

function kindOf(value: string): 'agent' | 'workspace' | 'sandbox' {
  for (const kind of ['agent', 'workspace', 'sandbox'] as const)
    if (value === `${kind}-profiles`) return kind;
  throw new Error(`Unknown Marketplace kind: ${value}`);
}
resourceDispatch['GET /api/v1/marketplace/{kind}'] = ({ client, param, options }) =>
  client.marketplace[kindOf(param('kind'))].list(options);
resourceDispatch['GET /api/v1/marketplace/{kind}/{id}'] = ({ client, param }) =>
  client.marketplace[kindOf(param('kind'))].read(param('id'));
resourceDispatch['POST /api/v1/marketplace/{kind}/{id}/fork'] = ({
  client,
  param,
  body,
  command,
}) => client.marketplace[kindOf(param('kind'))].fork(param('id'), body, command);
