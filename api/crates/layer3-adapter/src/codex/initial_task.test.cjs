const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { createServer } = require('node:http');
const { createInterface } = require('node:readline');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
const { stop } = require('./native_tools_fixture.cjs');

function connection(program, directory, home, binary) {
  const child = spawn(process.execPath, [program], {
    cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
    detached: true,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  child.stderr.resume();
  const frames = [];
  const pending = new Map();
  const listeners = new Set();
  let serial = 0;
  const lines = createInterface({ input: child.stdout });
  lines.on('line', (line) => {
    const frame = JSON.parse(line);
    if (frame.method || frame.error) frames.push(frame);
    pending.get(frame.id)?.(frame);
    for (const listener of listeners) listener();
  });
  const send = (id, method, params) => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  return {
    frames,
    discardResponse(method, params) { send(++serial, method, params); },
    async call(method, params) {
      const id = ++serial;
      let timer;
      const promise = new Promise((resolve) => pending.set(id, resolve));
      send(id, method, params);
      try {
        const frame = await Promise.race([promise, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error('Native task deadline: ' + method)), 30000);
        })]);
        assert.equal(frame.error, undefined, JSON.stringify(frame.error));
        return frame.result;
      } finally { clearTimeout(timer); pending.delete(id); }
    },
    async waitFor(predicate) {
      if (predicate(frames)) return;
      await new Promise((resolve, reject) => {
        const receive = () => {
          if (predicate(frames)) { clearTimeout(timer); listeners.delete(receive); resolve(); }
        };
        const timer = setTimeout(() => { listeners.delete(receive); reject(new Error('Native task observation deadline')); }, 30000);
        listeners.add(receive);
      });
    },
    async close() { lines.close(); await stop(child); },
  };
}

test('pinned native provider receives exact first task and recovers its unique result after a lost prompt response', {
  timeout: 120000,
  skip: !process.env.CMA_CODEX_TEST_BINARY || !process.env.CMA_ACP_TEST_BRIDGE,
}, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const program = process.env.CMA_ACP_TEST_BRIDGE;
  assert.match(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  assert(['84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c', '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4'].includes(createHash('sha256').update(readFileSync(program)).digest('hex')));
  const directory = mkdtempSync(join(tmpdir(), 'cma-initial-task-'));
  const home = join(directory, 'agent-home');
  mkdirSync(home);
  const input = '  比较三个工具\n保留原文。  ';
  const answer = 'Three tools compared for the isolated task.';
  const received = [];
  let boundaryError;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/responses');
      let body = '';
      for await (const chunk of request) body += chunk;
      const value = JSON.parse(body);
      received.push(value);
      const userText = value.input.filter((item) => item.role === 'user').flatMap((item) => item.content).filter((item) => item.type === 'input_text').map((item) => item.text);
      assert(userText.includes(input), JSON.stringify(userText));
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = (event) => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const id = 'resp_initial_task';
      const item = { id: 'msg_initial_task', type: 'message', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: answer, annotations: [] }] };
      send({ type: 'response.created', response: { id, status: 'in_progress', output: [] } });
      send({ type: 'response.output_item.added', output_index: 0, item: { ...item, status: 'in_progress' } });
      send({ type: 'response.output_text.delta', item_id: item.id, output_index: 0, content_index: 0, delta: answer });
      send({ type: 'response.output_item.done', output_index: 0, item });
      send({ type: 'response.completed', response: { id, status: 'completed', output: [item], usage: { input_tokens: 10, output_tokens: 8, total_tokens: 18 } } });
      response.end();
    } catch (error) {
      boundaryError = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  writeFileSync(join(home, 'config.toml'), [
    'model = "gpt-5.6-sol"', 'model_provider = "cma_test"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.cma_test]', 'name = "Isolated task proof"',
    'base_url = "http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api = "responses"', 'requires_openai_auth = false',
  ].join('\n'));
  let bridge;
  const open = async () => {
    bridge = connection(program, directory, home, binary);
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
  };
  const messages = (frames, kind) => frames.filter((frame) => frame.params?.update?.sessionUpdate === kind).map((frame) => frame.params.update.content?.text).filter((text) => text !== undefined);
  try {
    await open();
    const session = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    bridge.discardResponse('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text: input }] });
    try {
      await bridge.waitFor((frames) => messages(frames, 'agent_message_chunk').join('') === answer);
    } catch (error) {
      if (boundaryError) throw boundaryError;
      throw new Error('The pinned provider did not produce the isolated task result', { cause: error });
    }
    await bridge.close();
    await open();
    await bridge.call('session/load', { sessionId: session.sessionId, cwd: directory, mcpServers: [] });
    if (boundaryError) throw boundaryError;
    assert.deepEqual(messages(bridge.frames, 'user_message_chunk'), [input]);
    assert.equal(messages(bridge.frames, 'agent_message_chunk').join(''), answer);
    assert.equal(received.length, 1);
  } finally {
    await bridge?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
