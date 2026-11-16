import {define, acquire, egRequest, peer, same, digest, segment, invoke} from './helpers.mjs';

function content(ctx, envelope) {
  ctx.assert(!envelope.error && envelope.result?.isError !== true, 'mcp_tool_success');
  const value = envelope.result?.structuredContent;
  if (value) return value;
  const text = envelope.result?.content?.find(item => item.type === 'text')?.text;
  ctx.assert(typeof text === 'string', 'mcp_result_content');
  return JSON.parse(text);
}

export const mcpCases = [define(69, 'eg', ['eg'], async ctx => {
  let lease;
  const initialized = [], names = [];
  const rpc = async (replica, id, method, params, actor = 'agent') => {
    const result = await egRequest(ctx, lease, {method: 'POST', path: '/mcp', actor, replica,
      body: {jsonrpc: '2.0', id, method, params}, protocolVersion: '2025-06-18'});
    ctx.assert(result.jsonrpc === '2.0' && result.id === id, 'mcp_response_correlation');
    return result;
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'mcp_tools');
    for (const replica of [ctx.am, peer(ctx.am)]) {
      const result = await rpc(replica, 1, 'initialize', {protocolVersion: '2025-06-18',
        capabilities: {}, clientInfo: {name: 'eg-local-verifier', version: '1'}});
      ctx.assert(!result.error && result.result?.protocolVersion === '2025-06-18'
        && result.result.serverInfo.name === 'cmaeg' && result.result.capabilities.tools,
      'mcp_supported_initialization'); initialized.push(replica);
    }
  });
  await ctx.step('step_2', async () => {
    for (const replica of initialized) {
      const catalog = await rpc(replica, 2, 'tools/list', {});
      ctx.assert(Array.isArray(catalog.result?.tools), 'mcp_tools_catalog');
      const tools = catalog.result.tools;
      const capabilities = await egRequest(ctx, lease, {path: '/v1/channel-capabilities', replica});
      ctx.assert(same(tools.map(t => t.name).sort(), capabilities.agent_tools.map(t => t.name).sort()),
        'mcp_rest_catalog_agreement');
      ctx.assert(lease.expected.disabledTools.every(name => !tools.some(tool => tool.name === name)),
        'mcp_disabled_tools_absent'); names.push(tools.map(t => t.name).sort());
      const context = content(ctx, await rpc(replica, 3, 'tools/call', {
        name: 'readEventContext', arguments: {event_ref: lease.event.eventRef}}));
      const rest = await egRequest(ctx, lease, {path: `/v1/events/${segment(lease.event.eventRef)}/context`,
        actor: 'agent', replica});
      ctx.assert(same(context, rest) && context.event_id === lease.event.eventId, 'mcp_rest_context_scope');
      const key = lease.keys[replica], body = {text: `synthetic MCP reply ${replica}`};
      const reply = content(ctx, await rpc(replica, 4, 'tools/call', {name: 'replyToEvent',
        arguments: {event_ref: lease.event.eventRef, message: body, idempotency_key: key}}));
      ctx.assert(reply.delivery?.outcome === 'confirmed', 'mcp_reply_confirmed');
      const replay = await egRequest(ctx, lease, {method: 'POST', actor: 'agent', replica: peer(replica), key,
        path: `/v1/events/${segment(lease.event.eventRef)}/replies`, body});
      ctx.assert(same(replay, reply), 'mcp_rest_shared_reply_receipt');
      const denied = await rpc(replica, 5, 'tools/call', {name: 'readEventContext',
        arguments: {event_ref: lease.event.eventRef}}, 'wrong_consumer');
      ctx.assert(denied.error && !denied.result, 'mcp_wrong_consumer_refused');
    }
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(same(names[0], names[1]), 'mcp_two_replica_catalog');
    const effects = await invoke(lease, 'captureEffects', {kind: 'reply'});
    ctx.assert(effects.length === 2 && effects.every(e => e.destination === lease.event.destination),
      'mcp_exactly_once_scoped_replies');
    await ctx.observe('mcp_catalog_effects', async () => ({tools: names[0], effect_hashes: effects.map(digest)}));
  });
})];
