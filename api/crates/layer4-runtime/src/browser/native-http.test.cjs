const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { createServer } = require('node:http');
const { mkdir, readFile, writeFile, stat } = require('node:fs/promises');
const { join } = require('node:path');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');
const { runtimeFixture, conversation } = require('./native-http-fixture.cjs');

test('pinned native Agent uses the actual runtime HTTP conversation and mediated browser command', { timeout: 120000 }, async () => {
  const notifications = [];
  const navigated = [];
  let page = 'a';
  let modelRound = 0;
  let modelError;
  let expectedNavigation = false;
  const server = createServer(async (request, response) => {
    try {
      if (request.method === 'GET') {
        const visited = request.url.slice(1);
        if (!['a', 'b'].includes(visited)) { response.writeHead(404).end(); return; }
        navigated.push(visited);
        response.setHeader('content-type', 'text/html');
        response.end(`<title>Page ${visited}</title><body style="background:${visited === 'a' ? '#fff0d4' : '#d4e4ff'}"><h1>PAGE ${visited.toUpperCase()}</h1></body>`);
        return;
      }
      assert.equal(request.url, '/v1/responses');
      let bytes = '';
      for await (const chunk of request) { bytes += chunk; assert(bytes.length < 2 * 1024 * 1024); }
      const value = JSON.parse(bytes);
      const declared = value.tools ?? value.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const execution = declared.find(tool => tool.name === 'functions')?.tools?.find(tool => tool.name === 'exec');
      assert(execution || declared.some(tool => tool.name === 'exec_command'), 'actual native execution tool required');
      const command = { cmd: '/usr/local/bin/cma-browser navigate ' + origin + '/' + page, yield_time_ms: 10000, max_output_tokens: 1000 };
      const first = modelRound++ % 2 === 0;
      if (!first) {
        const outputs = JSON.stringify(value.input.filter(item => ['custom_tool_call_output', 'function_call_output'].includes(item.type)));
        assert(outputs.includes(expectedNavigation ? 'navigated' : 'unavailable'), outputs);
      }
      const output = first ? [{ id: 'fc_' + modelRound, call_id: 'call_' + modelRound, status: 'completed',
        ...(execution ? { type: 'custom_tool_call', namespace: 'functions', name: 'exec',
          input: 'text(await tools.exec_command(' + JSON.stringify(command) + '));' }
          : { type: 'function_call', name: 'exec_command', arguments: JSON.stringify(command) }) }]
        : [{ type: 'message', id: 'msg_' + modelRound, role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'Browser attempt completed.', annotations: [] }] }];
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const identifier = 'resp_http_browser_' + modelRound;
      send({ type: 'response.created', response: { id: identifier, status: 'in_progress', output: [] } });
      for (const [index, item] of output.entries()) {
        send({ type: 'response.output_item.added', output_index: index, item });
        send({ type: 'response.output_item.done', output_index: index, item });
      }
      send({ type: 'response.completed', response: { id: identifier, status: 'completed', output,
        usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } } });
      response.end();
    } catch (error) { modelError = error; if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  let runtime;
  let bridge;
  try {
    runtime = await runtimeFixture(origin);
    assert.equal((await runtime.browser('status')).state, 'not_enabled');
    bridge = await conversation(runtime.origin, notifications);
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call('session/new', { cwd: runtime.cwd, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId: session.sessionId, modeId: 'agent-full-access' });
    const prompt = async () => {
      await bridge.call('session/prompt', { sessionId: session.sessionId,
        prompt: [{ type: 'text', text: 'Use the mediated browser to navigate to local page ' + page }] });
      if (modelError) throw modelError;
    };
    await prompt();
    assert.equal(navigated.length, 0, 'native helper cannot implicitly enable a browser');
    const enabled = await runtime.browser('enable');
    const frames = [];
    let budgets = [];
    if (process.env.CMA_BROWSER_POLICY_PROOF === '1') {
      assert.equal(enabled.state, 'unsupported');
      await prompt();
      const observed = await runtime.browser('observe', { observer: 'native-http' });
      assert.equal(observed.state, 'unsupported');
      assert.equal(observed.frame, null);
      assert.equal(observed.observers, 0);
      assert.equal(navigated.length, 0);
    } else {
      assert.equal(enabled.state, 'loading', JSON.stringify(enabled));
      expectedNavigation = true;
      for (const destination of ['a', 'b']) {
        page = destination;
        await prompt();
        assert(navigated.includes(page), 'actual native navigation must reach the HTTP page');
        const observed = await runtime.browser('observe', { observer: 'native-http', context_id: enabled.context_id });
        assert.equal(observed.state, 'live');
        assert.equal(observed.context_id, enabled.context_id);
        assert.equal(observed.location, 'Local page');
        assert(Buffer.from(observed.frame.data, 'base64').length <= 262144);
        frames.push(observed.frame);
        await runtime.browser('release', { observer: 'native-http', context_id: enabled.context_id });
      }
      assert(frames[1].sequence > frames[0].sequence);
      assert(Date.parse(frames[1].captured_at) - Date.parse(frames[0].captured_at) >= 1000);
      assert.notEqual(frames[1].data, frames[0].data);
      const recovered = await runtime.browser('observe', { observer: 'reconnected', context_id: enabled.context_id });
      assert.equal(recovered.context_id, enabled.context_id);
      assert.equal(recovered.frame.data, frames[1].data);
      await runtime.browser('release', { observer: 'reconnected', context_id: enabled.context_id });
      if (process.env.CMA_BROWSER_PROOF_DIRECTORY) budgets = await measure(runtime);
    }
    const terminals = notifications.filter(value => value.params?.update?.sessionUpdate === 'tool_call_update'
      && ['completed', 'failed'].includes(value.params.update.status));
    const completions = terminals.filter(value => value.params.update.status === 'completed').length;
    assert(terminals.length >= 2, 'Actual native tool attempts must terminate across the runtime conversation');
    if (process.env.CMA_BROWSER_POLICY_PROOF !== '1') assert(completions >= 2, 'Two real native navigations must succeed');
    if (process.env.CMA_BROWSER_PROOF_DIRECTORY) {
      const proof = process.env.CMA_BROWSER_PROOF_DIRECTORY;
      await mkdir(proof, { recursive: true });
      for (const [index, frame] of frames.entries()) await writeFile(join(proof, 'runtime-native-page-' + index + '.jpg'), Buffer.from(frame.data, 'base64'));
      await writeFile(join(proof, 'runtime-native-http.json'), JSON.stringify({
        platform: process.platform, node: process.version, provider: '0.147.0',
        runtimeSha256: createHash('sha256').update(await readFile(runtime.binary)).digest('hex'),
        boot: runtime.boot, context: enabled.context_id, nativeToolCompletions: completions,
        nativeToolFailures: terminals.length - completions,
        nativeModelCalls: modelRound, navigated, state: enabled.state,
        pages: frames.map(frame => ({ sequence: frame.sequence, capturedAt: frame.captured_at,
          sha256: createHash('sha256').update(Buffer.from(frame.data, 'base64')).digest('hex') })), budgets,
        boundary: 'Actual native Agent through runtime HTTP/ACP and private browser command; public service and Web are separate pending proof',
      }, null, 2));
    }
    await runtime.browser('disable');
  } finally {
    bridge?.close();
    await runtime?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

async function cgroupSample() {
  const root = process.env.CMA_CGROUP_DIRECTORY || '/sys/fs/cgroup';
  const identity = await stat(join(root, 'cpu.stat'));
  const cpu = await readFile(join(root, 'cpu.stat'), 'utf8');
  const usage = /^usage_usec (\d+)$/m.exec(cpu);
  assert(usage, 'Stable cgroup v2 CPU accounting required');
  return { identity: `${identity.dev}:${identity.ino}`, usageUsec: Number(usage[1]),
    memoryBytes: Number((await readFile(join(root, 'memory.current'), 'utf8')).trim()),
    time: process.hrtime.bigint() };
}

async function measure(runtime) {
  assert.equal(process.platform, 'linux', 'Required resource acceptance uses stable Linux cgroup accounting');
  const budgets = [];
  for (const [mode, count] of [['normal', 1], ['hidden', 0], ['closed', 0], ['multiple', 4]]) {
    const before = await runtime.browser('status');
    const start = await cgroupSample();
    let peakMemory = start.memoryBytes;
    let transferred = 0;
    for (let tick = 0; tick < 5; tick += 1) {
      for (let observer = 0; observer < count; observer += 1) {
        const result = await runtime.browser('observe', { observer: 'budget-' + observer });
        transferred += Buffer.byteLength(JSON.stringify(result));
      }
      peakMemory = Math.max(peakMemory, (await cgroupSample()).memoryBytes);
      await delay(1000);
    }
    const end = await cgroupSample();
    assert.equal(start.identity, end.identity, 'Cgroup identity changed during the window');
    assert(end.usageUsec >= start.usageUsec, 'Cumulative cgroup CPU moved backwards');
    const seconds = Number(end.time - start.time) / 1e9;
    const after = await runtime.browser('status');
    assert.equal(after.observers, count);
    assert(after.captures - before.captures <= 6);
    if (count === 0) assert.equal(after.captures, before.captures);
    budgets.push({ mode, elapsedSeconds: seconds, observers: count,
      captures: after.captures - before.captures, responseBytes: transferred,
      bytesPerSecond: transferred / seconds, peakCgroupMemoryBytes: Math.max(peakMemory, end.memoryBytes),
      cpuPercentOfOneCore: (end.usageUsec - start.usageUsec) / (seconds * 10000),
      cpuMeasurement: 'Cgroup v2 cumulative usage_usec over monotonic elapsed time; full proof container',
      memoryMeasurement: 'Sampled full cgroup memory.current; includes runtime, native Agent, browser and proof process',
      bandwidthMeasurement: 'Application observation JSON bytes; excludes public HTTP/TLS framing' });
    for (let observer = 0; observer < count; observer += 1) await runtime.browser('release', { observer: 'budget-' + observer });
  }
  return budgets;
}
