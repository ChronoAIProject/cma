const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');
const { bridgePath } = require('./persist_conversations.cjs');
const { processRpc } = require('./native_tools_fixture.cjs');
const { controlledBinary, control } = require('./native_control_fixture.cjs');

test('pinned native completion and interruption remain distinct before a replacement turn', { timeout: 60000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? '/usr/local/bin/codex';
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  const provider = execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim();
  assert.match(provider, /^codex-cli 0\.(147|165)\.0$/);
  const bridgeSha256 = createHash('sha256').update(readFileSync(program)).digest('hex');
  assert(['84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c', '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4'].includes(bridgeSha256));
  const directory = mkdtempSync(join(tmpdir(), 'cma-composer-completion-'));
  const home = join(directory, 'home');
  mkdirSync(home);
  let held = false;
  let requests = 0;
  const server = createServer(async (request, response) => {
    for await (const chunk of request) void chunk;
    requests += 1;
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    const send = (event) => response.write('data: ' + JSON.stringify(event) + '\n\n');
    const responseId = 'resp_completion_' + requests;
    send({ type: 'response.created', response: { id: responseId, status: 'in_progress', output: [] } });
    if (held) return;
    const message = {
      id: 'msg_completion_' + requests,
      type: 'message', role: 'assistant', status: 'completed',
      content: [{ type: 'output_text', text: 'Confirmed native completion.', annotations: [] }],
    };
    send({ type: 'response.output_item.added', output_index: 0, item: { ...message, status: 'in_progress' } });
    send({ type: 'response.output_item.done', output_index: 0, item: message });
    send({ type: 'response.completed', response: { id: responseId, status: 'completed', output: [message],
      usage: { input_tokens: 12, output_tokens: 4, total_tokens: 16 } } });
    response.end();
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  writeFileSync(join(home, 'config.toml'), [
    'model="gpt-6"', 'model_provider="cma"', 'web_search="disabled"',
    'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.cma]', 'name="Offline completion boundary"',
    'base_url="http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api="responses"', 'requires_openai_auth=false',
  ].join('\n'));
  const bridge = processRpc(process.execPath, [program], { cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: controlledBinary(directory, binary) } });
  let pending;
  try {
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    const prompt = (text) => bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text }] });
    const completed = await prompt('Complete the first turn.');
    assert.equal(completed.stopReason, 'end_turn');
    held = true;
    const before = requests;
    let settled = false;
    pending = prompt('Keep this turn active until stopped.');
    void pending.finally(() => { settled = true; });
    for (let attempt = 0; requests === before; attempt += 1) {
      assert(attempt < 100, 'native model request did not start');
      await delay(50);
    }
    assert.equal(settled, false);
    const active = await control(directory, 'thread/read', { threadId: sessionId, includeTurns: true });
    assert.equal(active.thread.turns.length, 2);
    assert.equal(active.thread.turns[1].status, 'inProgress');
    bridge.notify('session/cancel', { sessionId });
    const interrupted = await pending;
    assert.equal(interrupted.stopReason, 'cancelled');
    const stopped = await control(directory, 'thread/read', { threadId: sessionId, includeTurns: true });
    assert.equal(stopped.thread.turns[1].status, 'interrupted');
    held = false;
    const replacement = await prompt('Start a distinct replacement turn.');
    assert.equal(replacement.stopReason, 'end_turn');
    const final = await control(directory, 'thread/read', { threadId: sessionId, includeTurns: true });
    assert.deepEqual(final.thread.turns.map((turn) => turn.status), ['completed', 'interrupted', 'completed']);
    assert.equal(new Set(final.thread.turns.map((turn) => turn.id)).size, 3);
    if (process.env.CMA_COMPLETION_NATIVE_OUTPUT) writeFileSync(process.env.CMA_COMPLETION_NATIVE_OUTPUT,
      JSON.stringify({ provider, bridgeSha256, completed, interrupted, replacement,
        statuses: final.thread.turns.map((turn) => turn.status) }, null, 2) + '\n');
  } finally {
    await bridge.close();
    await pending?.catch(() => undefined);
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
