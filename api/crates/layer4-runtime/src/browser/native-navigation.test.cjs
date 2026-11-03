const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtemp, mkdir, writeFile, readFile, rm } = require('node:fs/promises');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');
const { processRpc } = require('../../../layer3-adapter/src/codex/native_tools_fixture.cjs');

test('pinned native Agent navigates A then B; fresh bounded frames retain context across observation reconnect', { timeout: 90000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const bridgePath = process.env.CMA_ACP_TEST_BRIDGE;
  assert(binary && bridgePath, 'Pinned provider and bridge must be explicitly supplied');
  assert.equal(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
  assert.equal(createHash('sha256').update(await readFile(bridgePath)).digest('hex'),
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c');
  const directory = await mkdtemp(join(tmpdir(), 'cma-browser-native-'));
  const home = join(directory, 'agent');
  await mkdir(home);
  let requestedPage = 'a';
  let modelRound = 0;
  let modelFailure;
  const navigated = [];
  const notifications = [];
  const mediator = spawn(process.execPath, [join(__dirname, 'mediator.cjs'), directory], {
    env: { ...process.env }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  let diagnostic = '';
  mediator.stderr.on('data', chunk => { diagnostic += chunk; });
  const pending = new Map();
  let serial = 0;
  createInterface({ input: mediator.stdout }).on('line', line => {
    const response = JSON.parse(line);
    pending.get(response.id)?.(response);
  });
  const call = async (operation, parameters = {}) => {
    const identifier = ++serial;
    let deadline;
    const response = new Promise(resolve => pending.set(identifier, resolve));
    mediator.stdin.write(JSON.stringify({ id: identifier, operation, ...parameters }) + '\n');
    try {
      const result = await Promise.race([response, new Promise((resolve, reject) => {
        deadline = setTimeout(() => reject(new Error('Mediator deadline: ' + diagnostic)), 20000);
      })]);
      assert.equal(result.error, undefined);
      return result.value;
    } finally { clearTimeout(deadline); pending.delete(identifier); }
  };
  const server = createServer(async (request, response) => {
    try {
      if (request.method === 'GET') {
        const page = request.url.slice(1);
        navigated.push(page);
        response.setHeader('content-type', 'text/html');
        response.end(`<title>Page ${page}</title><body style="background:${page === 'a' ? '#fff0d4' : '#d4e4ff'}"><h1>PAGE ${page.toUpperCase()}</h1></body>`);
        return;
      }
      assert.equal(request.url, '/v1/responses');
      let body = '';
      for await (const chunk of request) body += chunk;
      const input = JSON.parse(body);
      const declared = input.tools ?? input.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const functions = declared.flatMap(tool => tool.tools ?? [tool]);
      const tool = functions.find(tool => ['exec_command', 'shell_command', 'shell', 'exec'].includes(tool.name));
      assert(tool, 'native execution tool is declared: ' + JSON.stringify(declared.map(tool => tool.name)));
      const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
      const command = `${quote(process.execPath)} ${quote(join(__dirname, 'command.cjs'))} navigate ${quote(origin + '/' + requestedPage)}`;
      const argumentsValue = tool.name === 'shell' ? { command: ['/bin/sh', '-c', command] }
        : tool.name === 'shell_command' ? { command, timeout_ms: 10000 }
          : { cmd: command, yield_time_ms: 10000, max_output_tokens: 1000 };
      const first = modelRound++ % 2 === 0;
      const output = first ? [{ id: 'fc_' + modelRound, call_id: 'call_' + modelRound, status: 'completed',
        ...(tool.name === 'exec' ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions',
          input: `const entry = ALL_TOOLS.find(tool => tool.name === "exec_command"); if (!entry) throw new Error("Missing native execution tool"); text(await tools[entry.name](${JSON.stringify(argumentsValue)}));` }
          : { type: 'function_call', name: tool.name, arguments: JSON.stringify(argumentsValue) }) }]
        : [{ type: 'message', id: 'msg_' + modelRound, role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'Navigation complete.', annotations: [] }] }];
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const identifier = 'resp_browser_' + modelRound;
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
    assert.equal((await call('status')).state, 'not_enabled');
    const enabled = await call('enable');
    assert.equal(enabled.state, 'loading', JSON.stringify(enabled));
    await writeFile(join(home, 'config.toml'), [
      'model = "gpt-5.6-sol"', 'model_provider = "browser_test"', 'web_search = "disabled"',
      'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
      '[model_providers.browser_test]', 'name = "Local browser proof"',
      `base_url = "${origin}/v1"`, 'wire_api = "responses"', 'requires_openai_auth = false',
    ].join('\n'));
    bridge = processRpc(process.execPath, [bridgePath], {
      cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary,
        CMA_BROWSER_SOCKET: join(directory, 'browser.sock') },
    }, notification => notifications.push(notification));
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId: session.sessionId, modeId: 'agent-full-access' });
    const frames = [];
    for (const page of ['a', 'b']) {
      requestedPage = page;
      await bridge.call('session/prompt', { sessionId: session.sessionId,
        prompt: [{ type: 'text', text: 'Use the mediated browser to navigate to page ' + page }] });
      if (modelFailure) throw modelFailure;
      assert(navigated.includes(page), 'native Agent actually requested page ' + page + ': ' + JSON.stringify(notifications.filter(value => ['tool_call', 'tool_call_update'].includes(value.params?.update?.sessionUpdate))));
      const observed = await call('observe', { observer: 'native-proof' });
      assert.equal(observed.state, 'live');
      assert.equal(observed.context_id, enabled.context_id);
      assert.equal(observed.location, 'Local page');
      assert(observed.frame.data.length < 350000);
      frames.push(observed.frame);
      const released = await call('release', { observer: 'native-proof' });
      assert.equal(released.state, 'ready');
      assert.equal(released.frame, null);
      assert.equal((await call('status')).state, 'ready');
    }
    assert(frames[1].sequence > frames[0].sequence);
    assert(Date.parse(frames[1].captured_at) - Date.parse(frames[0].captured_at) >= 1000, 'navigation must not bypass the capture frequency bound');
    assert.notEqual(createHash('sha256').update(frames[0].data).digest('hex'),
      createHash('sha256').update(frames[1].data).digest('hex'));
    assert(notifications.some(value => value.params?.update?.sessionUpdate === 'tool_call'));
    const recovered = await call('observe', { observer: 'reconnected-proof' });
    assert.equal(recovered.context_id, enabled.context_id);
    assert.equal(recovered.frame.data, frames[1].data);
    await call('release', { observer: 'reconnected-proof' });
    let deliveries = 0;
    for (let attempt = 0; attempt < 20; attempt += 1) {
      const repeated = await call('observe', { observer: 'bounded-delivery' });
      if (repeated.frame) deliveries += 1;
    }
    assert(deliveries <= 16, 'repeated reads cannot deliver unbounded cached frames');
    await call('release', { observer: 'bounded-delivery' });
    await delay(1000);
    if (process.env.CMA_BROWSER_PROOF_DIRECTORY) {
      const budgets = [];
      for (const [mode, count] of [['normal', 1], ['hidden', 0], ['closed', 0], ['multiple', 4]]) {
        const before = await call('status');
        const started = Date.now();
        let transferred = 0;
        let peakMemory = 0;
        const sample = () => {
          const resources = processResources(mediator.pid);
          peakMemory = Math.max(peakMemory, resources.rssKiB);
        };
        for (let tick = 0; tick < 5; tick += 1) {
          for (let index = 0; index < count; index += 1) {
            const observation = await call('observe', { observer: 'budget-' + index });
            transferred += Buffer.byteLength(JSON.stringify(observation));
          }
          sample();
          await delay(1000);
        }
        sample();
        const after = await call('status');
        const seconds = (Date.now() - started) / 1000;
        assert(after.captures - before.captures <= 6);
        assert.equal(after.observers, count);
        if (count === 0) assert.equal(after.captures, before.captures);
        budgets.push({ mode, observers: after.observers, elapsedSeconds: seconds,
          captures: after.captures - before.captures, responseBytes: transferred,
          bytesPerSecond: transferred / seconds, peakProcessTreeRssKiB: peakMemory,
          cpuPercentOfOneCore: null,
          cpuMeasurement: 'Unknown: process-tree churn prevents stable cumulative CPU accounting',
          memoryMeasurement: 'Sum of sampled process RSS; shared pages can be counted more than once; not browser working memory' });
        for (let index = 0; index < count; index += 1) await call('release', { observer: 'budget-' + index });
      }
      const proof = process.env.CMA_BROWSER_PROOF_DIRECTORY;
      await mkdir(proof, { recursive: true });
      for (const [index, frame] of frames.entries()) await writeFile(join(proof, 'native-page-' + index + '.jpg'), Buffer.from(frame.data, 'base64'));
      await writeFile(join(proof, 'native-browser.json'), JSON.stringify({
        platform: process.platform, node: process.version, provider: '0.147.0',
        browser: execFileSync(process.env.CMA_BROWSER_BINARY, ['--version'], { encoding: 'utf8' }).trim(),
        context: enabled.context_id, modelCalls: modelRound,
        nativeToolCompletions: notifications.filter(value => value.params?.update?.sessionUpdate === 'tool_call_update' && value.params.update.status === 'completed').length,
        pages: frames.map(frame => ({ sequence: frame.sequence, capturedAt: frame.captured_at,
          sha256: createHash('sha256').update(Buffer.from(frame.data, 'base64')).digest('hex') })), budgets,
      }, null, 2));
    }
    assert.equal((await call('disable')).state, 'not_enabled');
  } finally {
    await bridge?.close();
    mediator.stdin.end();
    mediator.kill('SIGTERM');
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
});

function processResources(root) {
  const rows = execFileSync('ps', ['-axo', 'pid=,ppid=,rss='], { encoding: 'utf8' })
    .trim().split('\n').map(line => {
      const [pid, parent, memory] = line.trim().split(/\s+/);
      return { pid: Number(pid), parent: Number(parent), memory: Number(memory) };
    });
  const selected = new Set([root]);
  for (let previous = -1; previous !== selected.size;) {
    previous = selected.size;
    for (const row of rows) if (selected.has(row.parent)) selected.add(row.pid);
  }
  return rows.filter(row => selected.has(row.pid)).reduce((total, row) => ({
    rssKiB: total.rssKiB + row.memory,
  }), { rssKiB: 0 });
}
