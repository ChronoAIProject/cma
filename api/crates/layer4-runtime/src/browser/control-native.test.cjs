const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const { createHash, randomUUID } = require('node:crypto');
const { mkdtemp, mkdir, writeFile, readFile, rm } = require('node:fs/promises');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');
const { processRpc } = require('../../../layer3-adapter/src/codex/native_tools_fixture.cjs');
const { runtimeFixture, conversation } = require('./native-http-fixture.cjs');

test('acquired user lease writes a known form while concurrent native Agent input is fenced; return preserves the page', { timeout: 90000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const bridgePath = process.env.CMA_ACP_TEST_BRIDGE;
  assert(binary && bridgePath, 'Pinned provider and bridge must be supplied');
  assert.equal(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
  assert.equal(createHash('sha256').update(await readFile(bridgePath)).digest('hex'),
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c');
  const directory = await mkdtemp(join(tmpdir(), 'cma-control-native-'));
  const home = join(directory, 'agent');
  await mkdir(home);
  let runtime;
  const mediator = process.env.CMA_CONTROL_RUNTIME_HTTP ? undefined : spawn(process.execPath, [join(__dirname, 'mediator.cjs'), directory], {
    env: { ...process.env }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  mediator?.stderr.resume();
  const pending = new Map();
  let serial = 0;
  if (mediator) createInterface({ input: mediator.stdout }).on('line', line => {
    const response = JSON.parse(line);
    pending.get(response.id)?.(response);
  });
  const call = async (operation, parameters = {}) => {
    if (runtime) {
      if (operation !== 'control') return runtime.browser(operation, parameters);
      const result = await runtime.control({ t: 'browser_control', sandbox_id: 'browser-proof', runtime_generation: 1,
        boot_id: runtime.boot, agent_id: 'browser-agent', ...parameters });
      assert.equal(result.t, 'browser_control', JSON.stringify(result));
      return { ...result.control, context_id: result.context_id };
    }
    const identifier = ++serial;
    let deadline;
    const response = new Promise(resolve => pending.set(identifier, resolve));
    mediator.stdin.write(JSON.stringify({ id: identifier, operation, ...parameters }) + '\n');
    try {
      const result = await Promise.race([response, new Promise((resolve, reject) => {
        deadline = setTimeout(() => reject(new Error('Mediator deadline')), 20000);
      })]);
      assert.equal(result.error, undefined, JSON.stringify(result));
      return result.value;
    } finally { clearTimeout(deadline); pending.delete(identifier); }
  };
  let nativeCommand;
  let modelRound = 0;
  let modelFailure;
  const values = [];
  let pages = 0;
  const notifications = [];
  const server = createServer(async (request, response) => {
    try {
      if (request.url === '/form') {
        pages += 1;
        response.writeHead(200, { 'content-type': 'text/html' });
        response.end('<body style="margin:20px"><h1>CONTROL PROOF</h1><input style="position:absolute;left:20px;top:70px;width:500px;height:40px" oninput="fetch(\'/value\',{method:\'POST\',body:this.value})"></body>');
        return;
      }
      let body = '';
      for await (const chunk of request) body += chunk;
      if (request.url === '/value') { values.push(body); response.end('ok'); return; }
      if (request.url === '/favicon.ico') { response.writeHead(404).end(); return; }
      assert.equal(request.url, '/v1/responses');
      const input = JSON.parse(body);
      const declared = input.tools ?? input.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const tool = declared.flatMap(tool => tool.tools ?? [tool]).find(tool => ['exec_command', 'shell_command', 'shell', 'exec'].includes(tool.name));
      assert(tool, 'Native execution tool must exist');
      const args = tool.name === 'shell' ? { command: ['/bin/sh', '-c', nativeCommand] }
        : tool.name === 'shell_command' ? { command: nativeCommand, timeout_ms: 10000 }
          : { cmd: nativeCommand, yield_time_ms: 10000, max_output_tokens: 1000 };
      const first = modelRound++ % 2 === 0;
      const output = first ? [{ id: 'fc_' + modelRound, call_id: 'call_' + modelRound, status: 'completed',
        ...(tool.name === 'exec' ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions',
          input: `const entry = ALL_TOOLS.find(tool => tool.name === "exec_command"); if (!entry) throw new Error("Missing native execution tool"); text(await tools[entry.name](${JSON.stringify(args)}));` }
          : { type: 'function_call', name: tool.name, arguments: JSON.stringify(args) }) }]
        : [{ type: 'message', id: 'msg_' + modelRound, role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'Attempt complete.', annotations: [] }] }];
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const identifier = 'resp_control_' + modelRound;
      send({ type: 'response.created', response: { id: identifier, status: 'in_progress', output: [] } });
      for (const [index, item] of output.entries()) {
        send({ type: 'response.output_item.added', output_index: index, item });
        send({ type: 'response.output_item.done', output_index: index, item });
      }
      send({ type: 'response.completed', response: { id: identifier, status: 'completed', output,
        usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } } });
      response.end();
    } catch (error) { modelFailure = error; response.writeHead(500).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let bridge;
  try {
    if (process.env.CMA_CONTROL_RUNTIME_HTTP) runtime = await runtimeFixture(origin);
    const enabled = await call('enable');
    assert.equal(enabled.state, 'loading');
    await writeFile(join(home, 'config.toml'), [
      'model = "gpt-5.6-sol"', 'model_provider = "browser_test"', 'web_search = "disabled"',
      'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
      '[model_providers.browser_test]', 'name = "Local browser proof"',
      `base_url = "${origin}/v1"`, 'wire_api = "responses"', 'requires_openai_auth = false',
    ].join('\n'));
    bridge = runtime ? await conversation(runtime.origin, notifications) : processRpc(process.execPath, [bridgePath], {
      cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary,
        CMA_BROWSER_SOCKET: join(directory, 'browser.sock') },
    }, notification => notifications.push(notification));
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call('session/new', { cwd: runtime?.cwd ?? directory, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId: session.sessionId, modeId: 'agent-full-access' });
    const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
    const agent = async (operation, value) => {
      nativeCommand = `${quote(process.execPath)} ${quote(join(__dirname, 'command.cjs'))} ${operation} ${quote(value)}`;
      await bridge.call('session/prompt', { sessionId: session.sessionId,
        prompt: [{ type: 'text', text: 'Execute the bounded browser command once.' }] });
      if (modelFailure) throw modelFailure;
    };
    await agent('navigate', origin + '/form');
    assert.equal(pages, 1);
    let control = await call('control', { command: 'acquire', context_id: enabled.context_id,
      actor: 'owner-proof', client_id: 'client-one', operation_id: randomUUID(), expected_revision: 0 });
    assert.equal(control.state, 'user');
    const mutate = async (command, input) => {
      control = await call('control', { command, context_id: enabled.context_id,
        actor: 'owner-proof', client_id: 'client-one', operation_id: randomUUID(),
        expected_revision: control.revision, lease_token: control.lease_token, input });
      return control;
    };
    await mutate('input', { kind: 'click', x: 100, y: 90 });
    await Promise.all([mutate('input', { kind: 'text', text: 'USER-17' }), agent('input', JSON.stringify({ kind: 'text', text: '-FORBIDDEN' }))]);
    await delay(100);
    assert.equal(values.at(-1), 'USER-17');
    assert(JSON.stringify(notifications).includes('browser_control_conflict'), 'Native Agent must receive an explicit runtime fence');
    const frame = await call('observe', { observer: 'proof' });
    assert.equal(frame.context_id, enabled.context_id);
    await mutate('return');
    assert.equal(control.state, 'agent');
    await agent('input', JSON.stringify({ kind: 'text', text: '-AGENT' }));
    await delay(100);
    assert.equal(values.at(-1), 'USER-17-AGENT');
    assert.equal(pages, 1, 'No page reload or browser replacement on return');
    const returned = await call('observe', { observer: 'proof' });
    assert.equal(returned.context_id, enabled.context_id);
    if (process.env.CMA_CONTROL_PROOF_DIRECTORY) {
      await mkdir(process.env.CMA_CONTROL_PROOF_DIRECTORY, { recursive: true });
      await writeFile(join(process.env.CMA_CONTROL_PROOF_DIRECTORY, 'native-control.json'), JSON.stringify({
        context_id: enabled.context_id, values, pages, modelRound, control, notifications,
        transport: runtime ? 'actual-runtime-http' : 'mediator-pipe',
        runtime_sha256: runtime ? createHash('sha256').update(await readFile(runtime.binary)).digest('hex') : null,
      }, null, 2));
      await writeFile(join(process.env.CMA_CONTROL_PROOF_DIRECTORY, 'user.jpg'), Buffer.from(frame.frame.data, 'base64'));
      await writeFile(join(process.env.CMA_CONTROL_PROOF_DIRECTORY, 'returned.jpg'), Buffer.from(returned.frame.data, 'base64'));
    }
  } finally {
    await bridge?.close();
    if (mediator) {
      mediator.stdin.end();
      await new Promise(resolve => mediator.exitCode !== null ? resolve() : mediator.once('exit', resolve));
    }
    await runtime?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
});
