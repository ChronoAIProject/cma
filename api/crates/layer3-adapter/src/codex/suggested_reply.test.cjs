const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = require('node:fs');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
const { processRpc } = require('./native_tools_fixture.cjs');
const { controlledBinary, control } = require('./native_control_fixture.cjs');
const { bridgePath } = require('./persist_conversations.cjs');

test('pinned native provider retains the exact scenario suggestion once across process reload', { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? '/usr/local/bin/codex';
  assert.match(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), 'cma-suggestion-'));
  const home = join(directory, 'agent-home');
  mkdirSync(home);
  const requests = [];
  let failure;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/responses');
      let body = '';
      for await (const chunk of request) body += chunk;
      requests.push(JSON.parse(body));
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = (event) => response.write(`data: ${JSON.stringify(event)}\n\n`);
      const id = `resp_suggestion_${requests.length}`;
      const item = { id: `msg_suggestion_${requests.length}`, type: 'message', role: 'assistant', status: 'completed',
        content: [{ type: 'output_text', text: requests.length === 1 ? 'The comparison is complete.' : 'The criteria are explicit.', annotations: [] }] };
      send({ type: 'response.created', response: { id, status: 'in_progress', output: [] } });
      send({ type: 'response.output_item.added', output_index: 0, item: { ...item, status: 'in_progress' } });
      send({ type: 'response.output_item.done', output_index: 0, item });
      send({ type: 'response.completed', response: { id, status: 'completed', output: [item], usage: { input_tokens: 10, output_tokens: 3, total_tokens: 13 } } });
      response.end();
    } catch (error) {
      failure = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  writeFileSync(join(home, 'config.toml'), [
    'model = "gpt-5.6-sol"', 'model_provider = "cma_test"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"', '[model_providers.cma_test]',
    'name = "Bounded external model fixture"', `base_url = "http://127.0.0.1:${server.address().port}/v1"`,
    'wire_api = "responses"', 'requires_openai_auth = false',
  ].join('\n'));
  const native = controlledBinary(directory, binary);
  const open = async () => {
    const bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath], {
      cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: native },
    });
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    return bridge;
  };
  let bridge;
  try {
    bridge = await open();
    const session = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    const question = 'Compare PostgreSQL and SQLite.';
    const suggestion = 'Add explicit comparison criteria and explain the trade-offs.';
    for (const text of [question, suggestion]) {
      await bridge.call('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text }] });
    }
    if (failure) throw failure;
    assert.equal(requests.length, 2);
    assert.equal(requests[1].input.filter((item) => item.role === 'user').flatMap((item) => item.content).filter((part) => part.text === suggestion).length, 1);
    await bridge.close();
    bridge = await open();
    await bridge.call('session/load', { sessionId: session.sessionId, cwd: directory, mcpServers: [] });
    const history = await control(directory, 'thread/read', { threadId: session.sessionId, includeTurns: true });
    const messages = history.thread.turns.flatMap((turn) => turn.items).filter((item) => item.type === 'userMessage');
    assert.deepEqual(messages.map((item) => item.content.map((part) => part.text).join('')), [question, suggestion]);
    assert.equal(requests.length, 2);
  } finally {
    await bridge?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
