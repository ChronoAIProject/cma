const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { randomBytes, createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
const { processRpc } = require('./native_tools_fixture.cjs');

async function fixture() {
  const capability = randomBytes(32).toString('hex');
  const state = { calls: [], models: [], errors: [], round: 0, phase: 'old' };
  async function handler(label, request, response) {
    try {
      if (request.method !== 'POST') { response.writeHead(405).end(); return; }
      let raw = ''; for await (const chunk of request) { raw += chunk; assert(raw.length < 2097152); }
      const value = JSON.parse(raw);
      if (request.url === '/api/v1/agent-tools') {
        assert.equal(request.headers.authorization, 'Bearer ' + capability);
        if (!Object.hasOwn(value, 'id')) { response.writeHead(202).end(); return; }
        let result;
        if (value.method === 'initialize') result = { protocolVersion: value.params.protocolVersion,
          serverInfo: { name: 'endpoint-migration-fixture', version: '1' }, capabilities: { tools: {} } };
        else if (value.method === 'tools/list') result = { tools: [{ name: 'endpoint_probe',
          description: 'Read the endpoint identity of this isolated test server.',
          inputSchema: { type: 'object', properties: {}, additionalProperties: false },
          annotations: { readOnlyHint: true, destructiveHint: false } }] };
        else if (value.method === 'tools/call') {
          assert.equal(value.params.name, 'endpoint_probe');
          state.calls.push({ phase: state.phase, endpoint: label });
          result = { content: [{ type: 'text', text: 'Fixture endpoint: ' + label }] };
        } else assert.fail('Unexpected fixture method');
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ jsonrpc: '2.0', id: value.id, result })); return;
      }
      assert.equal(request.url, '/v1/responses');
      state.models.push(value);
      const declared = value.tools ?? value.input.filter(i => i.type === 'additional_tools').flatMap(i => i.tools);
      const exec = declared.find(t => t.name === 'functions')?.tools?.find(t => t.name === 'exec');
      const exact = 'mcp__cma_access__endpoint_probe';
      const toolRound = state.round++ === 0;
      const item = toolRound ? { id: 'fc_' + state.models.length, call_id: 'call_' + state.models.length,
        status: 'completed', ...(exec ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions',
          input: 'const entry = ALL_TOOLS.find(t => t.name === ' + JSON.stringify(exact) + '); if (!entry) throw new Error("Tool missing"); text(await tools[entry.name]({}));' }
          : declared.some(t => t.name === exact) ? { type: 'function_call', name: exact, arguments: '{}' }
          : { type: 'function_call', name: 'endpoint_probe', namespace: 'mcp__cma_access', arguments: '{}' }) }
        : { id: 'msg_' + state.models.length, type: 'message', role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'ORIGINAL_CONVERSATION_HISTORY_MARKER', annotations: [] }] };
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const id = 'resp_' + state.models.length;
      send({ type: 'response.created', response: { id, status: 'in_progress', output: [] } });
      send({ type: 'response.output_item.added', output_index: 0, item: { ...item, status: 'in_progress' } });
      send({ type: 'response.output_item.done', output_index: 0, item });
      send({ type: 'response.completed', response: { id, status: 'completed', output: [item],
        usage: { input_tokens: 2, output_tokens: 1, total_tokens: 3 } } }); response.end();
    } catch (error) { state.errors.push(error.message); if (!response.headersSent) response.writeHead(500); response.end(); }
  }
  const servers = ['old', 'new'].map(label => createServer((req, res) => handler(label, req, res)));
  for (const server of servers) await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origins = servers.map(server => 'http://127.0.0.1:' + server.address().port);
  return { state, origins, tools: index => [{ type: 'http', name: 'cma_access',
    url: origins[index] + '/api/v1/agent-tools', headers: [{ name: 'Authorization', value: 'Bearer ' + capability }] }],
    async close() { for (const server of servers) { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } } };
}

test('idle endpoint migration retains native conversation, history, capability, settings and process', { timeout: 150000 }, async () => {
  const binary = '/usr/local/bin/codex';
  const bridgePath = '/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js';
  const version = execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim();
  assert.match(version, /^codex-cli 0\.(147|165)\.0$/);
  const dir = mkdtempSync(join(tmpdir(), 'cma-endpoint-proof-')); const home = join(dir, 'agent-home'); mkdirSync(home);
  const fixtureServer = await fixture(); const state = fixtureServer.state;
  writeFileSync(join(home, 'config.toml'), ['model="gpt-6"', 'model_reasoning_effort="high"', 'model_provider="cma"',
    'web_search="disabled"', 'approval_policy="never"', 'sandbox_mode="danger-full-access"', '[model_providers.cma]',
    'name="Isolated endpoint fixture"', 'base_url="' + fixtureServer.origins[0] + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false'].join('\n'));
  const owner = 'PRESERVED_OWNER_INSTRUCTIONS'; writeFileSync(join(home, 'AGENTS.md'), owner);
  writeFileSync(join(dir, 'dirty.txt'), 'PRESERVED_DIRTY_WORK');
  const bridge = processRpc(process.execPath, [bridgePath], { cwd: dir,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary } });
  const childPids = () => {
    const pending = [bridge.pid], found = [];
    while (pending.length) {
      const pid = pending.pop();
      try {
        const command = readFileSync('/proc/' + pid + '/cmdline', 'utf8');
        if (pid === bridge.pid || command.includes('app-server')) found.push(String(pid));
        const children = readFileSync('/proc/' + pid + '/task/' + pid + '/children', 'utf8').trim();
        if (children) pending.push(...children.split(/\s+/).map(Number));
      } catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    return found.sort();
  };
  try {
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const created = await bridge.call('session/new', { cwd: dir, mcpServers: fixtureServer.tools(0) });
    const prompt = async phase => { state.phase = phase; state.round = 0;
      await bridge.call('session/prompt', { sessionId: created.sessionId, prompt: [{ type: 'text', text: 'Run the isolated endpoint fixture.' }] });
      assert.deepEqual(state.errors, []); };
    await prompt('initial'); assert.deepEqual(state.calls, [{ phase: 'initial', endpoint: 'old' }]);
    const pids = childPids(); assert(pids.length >= 2);
    // Prove why a database/config-only update is insufficient for the live thread.
    await bridge.call('session/load', { sessionId: created.sessionId, cwd: dir, mcpServers: fixtureServer.tools(1) });
    await prompt('load-without-close'); assert.equal(state.calls.at(-1).endpoint, 'old');
    // Only settled fixture turns are refreshed. Repeating close simulates a lost ACK.
    await bridge.call('session/close', { sessionId: created.sessionId });
    await bridge.call('session/close', { sessionId: created.sessionId });
    await bridge.call('session/load', { sessionId: created.sessionId, cwd: dir, mcpServers: fixtureServer.tools(1) });
    await bridge.call('session/set_model', { sessionId: created.sessionId, modelId: 'gpt-6[high]' });
    await bridge.call('session/set_config_option', { sessionId: created.sessionId, configId: 'reasoning_effort', value: 'high' });
    await bridge.call('session/set_mode', { sessionId: created.sessionId, modeId: 'agent-full-access' });
    const before = state.models.length; await prompt('migrated');
    assert.equal(state.calls.at(-1).endpoint, 'new');
    const migrated = state.models[before]; assert(JSON.stringify(migrated).includes('ORIGINAL_CONVERSATION_HISTORY_MARKER'));
    assert(JSON.stringify(migrated).includes(owner));
    assert.equal(migrated.client_metadata.session_id, created.sessionId); assert.equal(migrated.client_metadata.thread_id, created.sessionId);
    assert.equal(migrated.model, 'gpt-6'); assert.equal(migrated.reasoning.effort, 'high');
    assert.deepEqual(childPids(), pids);
    assert.equal(readFileSync(join(dir, 'dirty.txt'), 'utf8'), 'PRESERVED_DIRTY_WORK');
    assert.equal(readFileSync(join(home, 'AGENTS.md'), 'utf8'), owner);
    // The exact old credential remains valid at the canonical transport endpoint.
    console.log(JSON.stringify({ version, bridgeSha256: createHash('sha256').update(readFileSync(bridgePath)).digest('hex'),
      calls: state.calls, sameConversation: true, sameProcesses: true, sameCapability: true, historyPreserved: true, ownerFilesPreserved: true }));
  } finally { await bridge.close(); await fixtureServer.close(); rmSync(dir, { recursive: true, force: true }); }
});
