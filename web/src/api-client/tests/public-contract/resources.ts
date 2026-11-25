import type { Dispatch } from './context.js';
import type { AgentProvenanceInventoryOptions } from '../../agents.js';
import type { AccessibleSandboxListOptions } from '../../sandboxes.js';

export const resourceDispatch: Record<string, Dispatch> = {
  'POST /api/v1/sandbox-creation-outcomes/lookup': ({ client, body, options }) =>
    client.sandboxes.lookupCreation(body, options),
  'POST /api/v1/telemetry/browser': ({ client, body }) =>
    client.reportBrowserFault(
      (body as { kind: import('../../browser-telemetry.js').BrowserFault }).kind,
    ),
  'GET /api/v1/agents/{id}/environment': ({ client, param }) =>
    client.agentFiles.environment(param('id')),
  'POST /api/v1/cmaeg/worker-credentials/classify': ({ client, body }) =>
    client.providerBindings.classifyWorkerCredential(body),
  'POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment':
    ({ client, param, fixture, query }) =>
      client.providerBindings.media.upload(
        param('binding_id'),
        param('conversation_id'),
        param('event_id'),
        Number(param('attachment_index')),
        new Blob([fixture.request.rawBody ?? ''], {
          type: fixture.request.headers['content-type'],
        }),
        {
          name: query.get('name')!,
          sha256: query.get('sha256')!,
          event_expires_at: Number(query.get('event_expires_at')),
          gateway_binding_generation: query.has('gateway_binding_generation')
            ? Number(query.get('gateway_binding_generation'))
            : undefined,
        },
      ),
  'GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment':
    ({ client, param }) =>
      client.providerBindings.media.get(
        param('binding_id'),
        param('conversation_id'),
        param('event_id'),
        Number(param('attachment_index')),
      ),
  'POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/supplement':
    ({ client, param, body }) =>
      client.providerBindings.media.supplement(
        param('binding_id'),
        param('conversation_id'),
        param('event_id'),
        Number(param('attachment_index')),
        body,
      ),
  'POST /api/v1/cmaeg/bindings/{binding_id}/preparation-inspections': ({ client, param, body }) =>
    client.providerBindings.inspectPreparation(param('binding_id'), body),
  'POST /api/v1/cmaeg/bindings/{binding_id}/preparations': ({ client, param, body, command }) =>
    client.providerBindings.prepare(param('binding_id'), body, command),
  'GET /api/v1/agents/{id}/desktop': ({ client, param }) => client.agents.desktop(param('id')),
  'GET /api/v1/agents/{id}/files/tree': ({ client, param, query }) =>
    client.agentFiles.list(param('id'), query.get('path') ?? ''),
  'GET /api/v1/agents/{id}/files/content': ({ client, param, query }) =>
    client.agentFiles.read(param('id'), query.get('path') ?? '', {
      maxBytes: query.has('max_bytes') ? Number(query.get('max_bytes')) : undefined,
    }),
  'GET /api/v1/agents/{id}/files/download': async ({ client, param, query }) =>
    new TextDecoder('utf-8', { fatal: true }).decode(
      await client.agentFiles.download(param('id'), query.get('path') ?? ''),
    ),
  'POST /api/v1/agents/{id}/files': ({ client, param, body }) =>
    client.agentFiles.write(param('id'), body),
  'DELETE /api/v1/agents/{id}/files': ({ client, param, query }) =>
    client.agentFiles.remove(
      param('id'),
      query.get('path') ?? '',
      query.get('recursive') === 'true',
    ),
  'GET /api/v1/agents/{id}/share-links': ({ client, param }) =>
    client.resourceAccess.listAgentShareLinks(param('id')),
  'POST /api/v1/agents/{id}/share-links': ({ client, param, body, command }) =>
    client.resourceAccess.createAgentShareLink(param('id'), body, command),
  'DELETE /api/v1/agents/{id}/share-links/{share_id}': ({ client, param, query, command }) =>
    client.resourceAccess.revokeAgentShareLink(
      param('id'),
      param('share_id'),
      Number(query.get('expected_revision')),
      command,
    ),
  'GET /api/v1/agents/{id}/share-recipients': ({ client, param, query }) =>
    client.resourceAccess.shareRecipients(param('id'), query.get('q') ?? ''),
  'POST /api/v1/agent-share-links/{share_id}/redeem': ({ client, param, body }) =>
    client.resourceAccess.redeemAgentShareLink(
      param('share_id'),
      (body as { token: string }).token,
    ),
  'POST /api/v1/agents/{id}/attachments': async ({
    client,
    param,
    body,
    query,
    command,
    fixture,
  }) => {
    const value = await client.agentAttachments.upload(
      param('id'),
      new Blob([body], { type: fixture.request.headers['content-type'] }),
      { ...command, name: query.get('name') ?? '' },
    );
    return {
      id: value.id,
      agent_id: value.agentId,
      name: value.name,
      path: value.path,
      size_bytes: value.size,
      media_type: value.mediaType,
      sha256: value.sha256,
    };
  },
  'GET /api/v1/agents/{id}/models': ({ client, param }) => client.agents.models(param('id')),
  'GET /api/v1/agents/{id}/skills': ({ client, param }) => client.agents.skills(param('id')),
  'POST /api/v1/agents/{id}/skills': ({ client, param, body, command }) =>
    client.agents.installSkill(param('id'), body, command),
  'PUT /api/v1/agents/{id}/service-access': ({ client, param, body, command }) =>
    client.agents.updateServiceAccess(param('id'), body, command),
  'GET /api/v1/agents/{id}/access-requests': ({ client, param }) =>
    client.agents.accessRequests(param('id')),
  'POST /api/v1/agents/{id}/access-requests/{request_id}/approve': ({
    client,
    param,
    body,
    command,
  }) => client.agents.approveAccessRequest(param('id'), param('request_id'), body, command),
  'POST /api/v1/agents/{id}/access-requests/{request_id}/deny': ({ client, param, command }) =>
    client.agents.denyAccessRequest(param('id'), param('request_id'), command),
  'GET /api/v1/agents/{id}/suggestions': ({ client, param, query }) =>
    client.agents.suggestions(param('id'), {
      turn_sequence: Number(query.get('turn_sequence')),
      answer_sequence: Number(query.get('answer_sequence')),
    }),
  'GET /api/v1/agents/{id}/message-feedback': ({ client, param, query }) =>
    client.agents.messageFeedback(param('id'), {
      ...(query.has('after') ? { after: Number(query.get('after')) } : {}),
      ...(query.has('limit') ? { limit: Number(query.get('limit')) } : {}),
    }),
  'GET /api/v1/agents/{id}/messages/{message_id}/feedback': ({ client, param }) =>
    client.agents.feedbackDetails(param('id'), param('message_id')),
  'PUT /api/v1/agents/{id}/messages/{message_id}/feedback/reasons': ({ client, param, body }) =>
    client.agents.setFeedbackReasons(param('id'), param('message_id'), body),
  'PUT /api/v1/agents/{id}/messages/{message_id}/feedback': ({ client, param, body }) =>
    client.agents.setMessageFeedback(
      param('id'),
      param('message_id'),
      (body as { vote: 'up' | 'down' | null }).vote,
    ),
  'GET /api/v1/agents/{id}/preferences': ({ client, param }) =>
    client.agents.preferences(param('id')),
  'PATCH /api/v1/agents/{id}/preferences': ({ client, param, body, command }) =>
    client.agents.updatePreferences(param('id'), body, command),
  'GET /api/v1/agents/{id}/settings': ({ client, param }) => client.agents.settings(param('id')),
  'PATCH /api/v1/agents/{id}/settings': ({ client, param, body, command }) =>
    client.agents.updateSettings(param('id'), body, command),
  'POST /api/v1/agents': ({ client, body, command }) =>
    client.agents.createLifecycle(body, command),
  'GET /api/v1/agent-lifecycle/{id}': ({ client, param }) =>
    client.agents.readLifecycle(param('id')),
  'POST /api/v1/agent-lifecycle/{id}/cancel': ({ client, param }) =>
    client.agents.cancelLifecycle(param('id')),
  'GET /api/v1/desktop-capability': ({ client, query }) =>
    client.agents.desktopCapability({
      workspace_id: query.get('workspace_id') ?? undefined,
      sandbox_id: query.get('sandbox_id') ?? undefined,
    }),
  'POST /api/v1/sandboxes/{id}/workspaces/inline': ({ client, param, body, command }) =>
    client.workspaces.createInline(param('id'), body, command),
  'PUT /api/v1/cmaeg/bindings/{binding_id}': ({ client, param, body }) =>
    client.providerBindings.put(param('binding_id'), body),
  'GET /api/v1/cmaeg/bindings/{binding_id}': ({ client, param }) =>
    client.providerBindings.get(param('binding_id')),
  'DELETE /api/v1/cmaeg/bindings/{binding_id}': ({ client, param }) =>
    client.providerBindings.delete(param('binding_id')),
  'GET /api/v1/cmaeg/bindings/{binding_id}/conversations': ({ client, param }) =>
    client.providerBindings.listConversations(param('binding_id')),
  'POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-drift': ({
    client,
    param,
    body,
  }) =>
    client.providerBindings.inspectDesiredProfile(
      param('binding_id'),
      param('conversation_id'),
      body,
    ),
  'POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-transfers': ({
    client,
    param,
    body,
    command,
  }) =>
    client.providerBindings.transferConversationProfile(
      param('binding_id'),
      param('conversation_id'),
      body,
      command,
    ),
  'PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}': ({ client, param }) =>
    client.providerBindings.ensureConversation(param('binding_id'), param('conversation_id')),
  'GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}': ({ client, param }) =>
    client.providerBindings.inspectConversation(param('binding_id'), param('conversation_id')),
  'POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets': ({
    client,
    param,
    body,
    command,
  }) =>
    client.providerBindings.resetConversation(
      param('binding_id'),
      param('conversation_id'),
      body,
      command,
    ),
  'DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}': ({
    client,
    param,
  }) => client.providerBindings.deleteConversation(param('binding_id'), param('conversation_id')),
  'GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}': ({
    client,
    param,
  }) =>
    client.responsesV2.readEventContext(
      param('binding_id'),
      param('conversation_id'),
      param('event_id'),
    ),
  'PUT /api/v1/gateway/channel-records/{record_id}': ({ client, param, body }) =>
    client.gatewayRecords.put(param('record_id'), body),
  'GET /api/v1/gateway/channel-records/{record_id}': ({ client, param }) =>
    client.gatewayRecords.get(param('record_id')),
  'GET /api/v1/gateway/channel-records': ({ client, query }) =>
    client.gatewayRecords.list({
      owner: query.get('owner') ?? undefined,
      after: query.get('after') ?? undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
    }),
  'DELETE /api/v1/gateway/channel-records/{record_id}': ({ client, param, query }) =>
    client.gatewayRecords.delete(
      param('record_id'),
      query.has('expected_version') ? Number(query.get('expected_version')) : undefined,
    ),
  'PUT /api/v1/gateway/connection-records/{record_id}': ({ client, param, body }) =>
    client.gatewayConnectionRecords.put(param('record_id'), body),
  'GET /api/v1/gateway/connection-records/{record_id}': ({ client, param }) =>
    client.gatewayConnectionRecords.get(param('record_id')),
  'GET /api/v1/gateway/connection-records': ({ client, query }) =>
    client.gatewayConnectionRecords.list({
      owner: query.get('owner') ?? undefined,
      after: query.get('after') ?? undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
    }),
  'DELETE /api/v1/gateway/connection-records/{record_id}': ({ client, param, query }) =>
    client.gatewayConnectionRecords.delete(
      param('record_id'),
      query.has('expected_version') ? Number(query.get('expected_version')) : undefined,
    ),
  'GET /api/v1/agents/{id}/provider-associations': ({ client, param, options }) =>
    client.agents.providerAssociations(param('id'), options),
  'GET /api/v1/agents/{id}/channel-usage': ({ client, param }) =>
    client.agents.channelUsage(param('id')),
  'GET /api/v1/agents/{id}/channel-usage/history': ({ client, param, options }) =>
    client.agents.channelUsageHistory(param('id'), options),
  'GET /api/v1/agents/{id}/artifacts': ({ client, param, options }) =>
    client.agents.listArtifacts(param('id'), options),
  'GET /api/v1/agents/{id}/artifacts/{artifact_id}': ({ client, param }) =>
    client.agents.readArtifact(param('id'), param('artifact_id')),
  'POST /api/v1/agents/{id}/artifacts': ({ client, param, body, command }) =>
    client.agents.publishArtifact(param('id'), body, command),
  'GET /api/v1/agents/{id}/artifacts/{artifact_id}/download': async ({ client, param }) =>
    new TextDecoder('utf-8', { fatal: true }).decode(
      await client.agents.downloadArtifact(param('id'), param('artifact_id')),
    ),
  'GET /api/v1/organizations': ({ client }) => client.resourceAccess.organizations(),
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
  'GET /api/v1/sandboxes/capacity': ({ client }) => client.sandboxes.capacity(),
  'GET /api/v1/sandboxes': ({ client, options, query }) =>
    client.sandboxes.listAccessible({
      ...(options as AccessibleSandboxListOptions),
      excludeRetired: query.has('exclude_retired')
        ? query.get('exclude_retired') === 'true'
          ? true
          : query.get('exclude_retired') === 'false'
            ? false
            : (query.get('exclude_retired') as never)
        : undefined,
      shared: query.has('shared')
        ? query.get('shared') === 'true'
          ? true
          : query.get('shared') === 'false'
            ? false
            : (query.get('shared') as never)
        : undefined,
    }),
  'POST /api/v1/sandboxes': ({ client, body, command }) => client.sandboxes.create(body, command),
  'GET /api/v1/sandboxes/{id}': ({ client, param }) => client.sandboxes.readAccessible(param('id')),
  'POST /api/v1/sandboxes/{id}/commands': ({ client, param, body, command }) =>
    client.sandboxes.submitCommand(param('id'), body, command),
  'GET /api/v1/sandbox-commands/{id}': ({ client, param }) =>
    client.sandboxes.readCommand(param('id')),
  'DELETE /api/v1/sandboxes/{id}': ({ client, param, command }) =>
    client.sandboxes.delete(param('id'), command),
  'POST /api/v1/sandboxes/{id}/pause': ({ client, param, command }) =>
    client.sandboxes.pause(param('id'), command),
  'POST /api/v1/sandboxes/{id}/revive': ({ client, param, command }) =>
    client.sandboxes.revive(param('id'), command),
  'POST /api/v1/sandboxes/{id}/runtime-update': ({ client, param, body, command }) =>
    client.sandboxes.updateRuntime(param('id'), body, command),
  'GET /api/v1/sandboxes/{id}/runtime-recovery': ({ client, param }) =>
    client.sandboxes.inspectRuntimeRecovery(param('id')),
  'POST /api/v1/sandboxes/{id}/runtime-recovery': ({ client, param, body, command }) =>
    client.sandboxes.recoverRetained(param('id'), body, command),
  'POST /api/v1/sandboxes/{id}/always-on': ({ client, param, body, command }) =>
    client.sandboxes.setAlwaysOn(param('id'), body, command),
  'POST /api/v1/sandboxes/{id}/service-access': ({ client, param, body, command }) =>
    client.sandboxes.grantServiceAccess(param('id'), body, command),
  'GET /api/v1/sandboxes/{id}/events': ({ client, param, options }) =>
    client.sandboxes.events(param('id'), options),
  'GET /api/v1/sandboxes/{id}/workspaces': ({ client, param, options }) =>
    client.workspaces.listAccessible(param('id'), options),
  'POST /api/v1/sandboxes/{id}/workspaces': ({ client, param, body, command }) =>
    client.workspaces.create(param('id'), body, command),
  'GET /api/v1/workspaces/{id}': ({ client, param }) =>
    client.workspaces.readAccessible(param('id')),
  'DELETE /api/v1/workspaces/{id}': ({ client, param, command }) =>
    client.workspaces.delete(param('id'), command),
  'POST /api/v1/workspaces/{id}/recover-prepare': ({ client, param, command }) =>
    client.workspaces.recoverPrepare(param('id'), command),
  'GET /api/v1/workspaces/{id}/worktrees': ({ client, param, options }) =>
    client.worktrees.listAccessible(param('id'), options),
  'GET /api/v1/worktrees/{id}': ({ client, param }) => client.worktrees.readAccessible(param('id')),
  'GET /api/v1/agents': ({ client, options }) => client.agents.listAccessible(options),
  'GET /api/v1/agents/provenance': ({ client, options, query }) =>
    client.agents.provenanceInventory({
      ...(options as AgentProvenanceInventoryOptions),
      shared: query.has('shared') ? query.get('shared') === 'true' : undefined,
      creationOrigin:
        (query.get('creation_origin') as AgentProvenanceInventoryOptions['creationOrigin']) ??
        undefined,
      channelActive: query.has('channel_active')
        ? query.get('channel_active') === 'true'
          ? true
          : query.get('channel_active') === 'false'
            ? false
            : (query.get('channel_active') as never)
        : undefined,
      channelHistory: query.has('channel_history')
        ? query.get('channel_history') === 'true'
          ? true
          : query.get('channel_history') === 'false'
            ? false
            : (query.get('channel_history') as never)
        : undefined,
    }),
  'GET /api/v1/agents/{id}': ({ client, param }) => client.agents.readAccessible(param('id')),
  'GET /api/v1/agents/{id}/provenance': ({ client, param }) =>
    client.agents.provenance(param('id')),
  'GET /api/v1/agents/{id}/source': ({ client, param }) =>
    client.agents.sourceAccessible(param('id')),
  'POST /api/v1/workspaces/{id}/agents': ({ client, param, body, command }) =>
    client.agents.create(param('id'), body, command),
  'DELETE /api/v1/agents/{id}': ({ client, param, command }) =>
    client.agents.delete(param('id'), command),
  'POST /api/v1/agents/{id}/recover-worktree': ({ client, param, command }) =>
    client.agents.recoverWorktree(param('id'), command),
  'POST /api/v1/agents/{id}/restart': ({ client, param, command }) =>
    client.agents.restart(param('id'), command),
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
  'GET /api/v1/agents/{id}/retained-output': ({ client, param, query }) =>
    client.agents.retainedOutput(param('id'), {
      availability: { available: true },
      cursor: query.get('cursor') ?? undefined,
    }),
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
