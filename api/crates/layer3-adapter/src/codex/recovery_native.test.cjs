const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const fs = require('node:fs');
const { createServer } = require('node:http');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { setTimeout: delay } = require('node:timers/promises');
const { test } = require('node:test');

require('./credit_boundary_native.test.cjs');

const evidence = process.env.CMA_EOF_EVIDENCE_ROOT || require('node:os').tmpdir();
const nativeTarget = process.arch === 'arm64' ? 'codex-linux-arm64/vendor/aarch64-unknown-linux-musl' : 'codex-linux-x64/vendor/x86_64-unknown-linux-musl';
const binary = process.env.CMA_CODEX_TEST_BINARY || '/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/' + nativeTarget + '/bin/codex';
const bridgeProgram = process.env.CMA_ACP_TEST_BRIDGE || require('./persist_conversations.cjs').bridgePath;
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
function assertReviewedNative() {
  assert(['cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40',
    'e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2',
    '19c4f144c5226a9f17c58e6f0fa854843b0f77a6eb420f40e2745a12f10f5d37',
    'c06a67fd8ac76508dc78bd99ce20c9f317b8d568beda59485f3f4134df9962d5',
    'b32ece31af06440da980b733fb8fefdd1f193439595fda84096f1caaceec543f',
    'a45099154d7fb4a5ed8a56f40966a95deb938e508bab23e6985035c709e9a85a',
    process.env.CMA_CODEX_TEST_NATIVE_SHA256].includes(sha(fs.readFileSync(binary))), 'native must match a reviewed producer pin');
}
const exists = path => fs.existsSync(path);
const alive = pid => { try { process.kill(pid, 0); return true; } catch (e) { if (e.code === 'ESRCH') return false; throw e; } };
async function until(predicate, label, deadlineMs = 30000) {
  const deadline = Date.now() + deadlineMs;
  while (!predicate()) { assert(Date.now() < deadline, 'Deadline: ' + label); await delay(50); }
}
function wrapper(directory, phase) {
  const statusPath = join(directory, phase + '-native-status.json');
  const pidPath = join(directory, phase + '-native.pid');
  const stderrPath = join(directory, phase + '-native-stderr.log');
  const launcher = join(directory, phase + '-native-launcher.py');
  const py = `#!/usr/bin/python3
import json,subprocess,sys,time
from pathlib import Path
stderr=open(${JSON.stringify(stderrPath)},'wb',buffering=0)
child=subprocess.Popen([${JSON.stringify(binary)},*sys.argv[1:]],stdin=sys.stdin,stdout=sys.stdout,stderr=stderr)
Path(${JSON.stringify(pidPath)}).write_text(str(child.pid))
code=child.wait()
Path(${JSON.stringify(statusPath)}).write_text(json.dumps({'nativePid':child.pid,'exitCode':code,'exitedAt':time.time()}))
sys.exit(code if code>=0 else 128-code)
`;
  fs.writeFileSync(launcher, py, { mode: 0o755 });
  return { launcher, statusPath, pidPath, stderrPath };
}
function connect(directory, home, phase) {
  const native = wrapper(directory, phase);
  const frames = [], sentFrames = [];
  const child = spawn(process.execPath, [bridgeProgram], {
    cwd: directory, detached: true,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: native.launcher, RUST_LOG: 'warn' },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const stderr = fs.createWriteStream(join(directory, phase + '-bridge-stderr.log'));
  child.stderr.pipe(stderr);
  const pending = new Map(); let serial = 0;
  const send = frame => { sentFrames.push(frame); child.stdin.write(JSON.stringify(frame) + '\n'); };
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    const frame = JSON.parse(line); frames.push(frame);
    if (frame.method === 'session/request_permission') {
      const option = frame.params.options.find(x => x.kind === 'allow_once');
      assert(option);
      send({ jsonrpc: '2.0', id: frame.id, result: { outcome: { outcome: 'selected', optionId: option.optionId } } });
    } else pending.get(frame.id)?.resolve(frame);
  });
  const exited = new Promise(resolve => child.once('exit', (code, signal) => {
    for (const p of pending.values()) p.reject(new Error('Bridge exited: ' + signal + '/' + code));
    resolve({ code, signal });
  }));
  return {
    child, native, frames, sentFrames, exited, phase,
    async call(method, params) {
      const id = ++serial; let timer;
      const promise = new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        timer = setTimeout(() => reject(new Error('RPC deadline: ' + method)), 30000);
      });
      send({ jsonrpc: '2.0', id, method, params });
      try { const frame = await promise; assert.equal(frame.error, undefined, JSON.stringify(frame.error)); return frame.result; }
      finally { clearTimeout(timer); pending.delete(id); }
    },
    async stopBridgeOnly() {
      const started = Date.now();
      assert(alive(child.pid));
      process.kill(child.pid, 'SIGTERM');
      const bridgeExit = await exited;
      assert.equal(bridgeExit.signal, 'SIGTERM');
      await until(() => exists(native.statusPath), 'native graceful exit after bridge EOF');
      const status = JSON.parse(fs.readFileSync(native.statusPath, 'utf8'));
      assert.equal(status.exitCode, 0, 'native must return normally, not receive a terminating signal');
      const log = fs.readFileSync(native.stderrPath, 'utf8');
      assert(!/failed to shutdown thread persistence|failed to submit Shutdown|shutdown timed out|timed out waiting for thread .* shut down/.test(log), 'native shutdown reported failure');
      fs.writeFileSync(join(directory, phase + '-acp-frames.json'), JSON.stringify(frames, null, 2));
      return { bridgePid: child.pid, ...status, elapsedMs: Date.now() - started, stderrSha256: sha(log) };
    },
  };
}
function historyFiles(root) {
  if (!exists(root)) return [];
  return fs.readdirSync(root, { withFileTypes: true }).flatMap(x => x.isDirectory() ? historyFiles(join(root, x.name)) : x.name.endsWith('.jsonl') ? [join(root, x.name)] : []);
}
function history(home, sessionId) {
  const candidates = historyFiles(join(home, 'sessions')).map(path => {
    const bytes = fs.readFileSync(path);
    assert(bytes.length && bytes.at(-1) === 10, 'persisted history must end in a complete line');
    const records = bytes.toString('utf8').trim().split('\n').map(line => JSON.parse(line));
    return { path, bytes, records };
  });
  const matches = candidates.filter(x => x.records.some(r => r.type === 'session_meta' && r.payload.id === sessionId));
  assert.equal(matches.length, 1);
  return matches[0];
}
function complete(response, id, output) {
  const send = value => response.write('data: ' + JSON.stringify(value) + '\n\n');
  send({ type: 'response.created', response: { id, status: 'in_progress', output: [] } });
  output.forEach((item, output_index) => {
    send({ type: 'response.output_item.added', output_index, item: { ...item, status: 'in_progress' } });
    send({ type: 'response.output_item.done', output_index, item });
  });
  send({ type: 'response.completed', response: { id, status: 'completed', output, usage: { input_tokens: 10, output_tokens: 3, total_tokens: 13 } } });
  response.end();
}
const message = (id, text) => ({ id, type: 'message', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text, annotations: [] }] });

test('bridge-only SIGTERM gives pinned native stdio EOF, flushes interrupted turn, stops shell, and preserves same conversation', { timeout: 120000 }, async () => {
  assert.match(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  assert(['113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4',
    '4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888',
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c',
    'ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66',
    'fc9f74358c6ce5aeda4ac164c1daf83c513fb0f18df4b75f3d20bd0816fc6fa7',
    '8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4',
    '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4',
    'ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1',
    'e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484'].includes(sha(fs.readFileSync(bridgeProgram))), 'bridge must match reviewed source');
  assertReviewedNative();
  const directory = fs.mkdtempSync(join(evidence, 'cma-graceful-eof-'));
  const home = join(directory, 'home'); fs.mkdirSync(home);
  const dirtyPath = join(directory, 'dirty.txt');
  const toolPidPath = join(directory, 'tool.pid');
  const receipts = { binarySha256: sha(fs.readFileSync(binary)), bridgeSha256: sha(fs.readFileSync(bridgeProgram)), directory };
  let modelCalls = 0, serverError, phase = 'original', stage = 'initialize', failure;
  const bridges = [];
  const requests = [];
  const server = createServer(async (request, response) => {
    try {
      let body = ''; for await (const chunk of request) body += chunk;
      assert.equal(request.url, '/v1/responses');
      const value = JSON.parse(body); requests.push(value); modelCalls++;
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const id = 'resp_eof_' + modelCalls;
      if (modelCalls === 1 || phase === 'reloaded') {
        complete(response, id, [message('msg_eof_' + modelCalls, phase === 'reloaded' ? 'Recovered same conversation.' : 'Completed before recovery.')]);
      } else if (modelCalls === 2) {
        const code = 'import os,time,pathlib; pathlib.Path(' + JSON.stringify(dirtyPath) + ').write_text("native dirty survivor\\n"); pathlib.Path(' + JSON.stringify(toolPidPath) + ').write_text(str(os.getpid())); time.sleep(300)';
        const command = '/usr/bin/python3 -c ' + "'" + code.replaceAll("'", "'\\''") + "'";
        const params = { cmd: command, yield_time_ms: 1000, max_output_tokens: 1000 };
        const declared = value.tools || (value.input || []).filter(x => x.type === 'additional_tools').flatMap(x => x.tools);
        const host = declared.find(t => t.name === 'functions')?.tools?.find(t => t.name === 'exec');
        const direct = declared.find(t => t.name === 'exec_command');
        assert(host || direct, 'shell tool unavailable: ' + JSON.stringify(declared.map(t => t.name)));
        const item = host ? { id: 'fc_shell', call_id: 'call_shell', type: 'custom_tool_call', name: 'exec', namespace: 'functions', input: 'text(await tools.exec_command(' + JSON.stringify(params) + '));', status: 'completed' } : { id: 'fc_shell', call_id: 'call_shell', type: 'function_call', name: 'exec_command', arguments: JSON.stringify(params), status: 'completed' };
        complete(response, id, [item]);
      } else {
        response.write('data: ' + JSON.stringify({ type: 'response.created', response: { id, status: 'in_progress', output: [] } }) + '\n\n');
      }
    } catch (error) { serverError = error; if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  fs.writeFileSync(join(home, 'config.toml'), [
    'model="gpt-5.6-sol"', 'model_provider="cma_eof_fixture"', 'web_search="disabled"', 'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.cma_eof_fixture]', 'name="Offline EOF fixture"', 'base_url="http://127.0.0.1:' + server.address().port + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
  ].join('\n'));
  let bridge, pendingPrompt, toolPid;
  try {
    bridge = connect(directory, home, 'original'); bridges.push(bridge);
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call('session/new', { cwd: directory, mcpServers: [] }); receipts.sessionId = sessionId;
    // ACP applies its session mode to every turn, overriding config.toml.
    // Select full access explicitly: the runtime container cannot nest the
    // default workspace-write Linux sandbox when launching the fixture shell.
    await bridge.call('session/set_mode', { sessionId, modeId: 'agent-full-access' });
    stage = 'completed checkpoint';
    await bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Record the completed checkpoint.' }] });
    stage = 'active native turn and shell';
    pendingPrompt = bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Create the fixture dirty file and keep the turn waiting.' }] });
    void pendingPrompt.catch(() => {});
    await until(() => { if (serverError) throw serverError; return modelCalls >= 3 && exists(toolPidPath); }, 'active native turn and shell');
    toolPid = Number(fs.readFileSync(toolPidPath, 'utf8')); assert(alive(toolPid));
    receipts.before = { modelCalls, toolPid, dirtySha256: sha(fs.readFileSync(dirtyPath)), nativePid: Number(fs.readFileSync(bridge.native.pidPath, 'utf8')) };
    stage = 'first graceful shutdown';
    receipts.firstShutdown = await bridge.stopBridgeOnly();
    await pendingPrompt.catch(() => {});
    await until(() => !alive(toolPid), 'native shutdown must stop unified-exec shell');
    const firstHistory = history(home, sessionId);
    const events = firstHistory.records.filter(r => r.type === 'event_msg').map(r => r.payload.type);
    assert.equal(events.filter(x => x === 'task_started').length, 2);
    assert(events.includes('task_complete')); assert(events.includes('turn_aborted'));
    assert(firstHistory.bytes.includes('Completed before recovery.'));
    assert(firstHistory.bytes.includes('Create the fixture dirty file'));
    assert.equal(sha(fs.readFileSync(dirtyPath)), receipts.before.dirtySha256);
    receipts.afterShutdown = { historyPath: firstHistory.path, historySha256: sha(firstHistory.bytes), historyBytes: firstHistory.bytes.length, eventTypes: events, toolGone: !alive(toolPid) };
    stage = 'cold reload';
    phase = 'reloaded'; bridge = connect(directory, home, 'reloaded'); bridges.push(bridge);
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const callsBeforeLoad = modelCalls;
    await bridge.call('session/load', { sessionId, cwd: directory, mcpServers: [] });
    assert.equal(modelCalls, callsBeforeLoad, 'cold load without a goal must not request model work');
    assert(JSON.stringify(bridge.frames).includes('Completed before recovery.'));
    await bridge.call('session/set_mode', { sessionId, modeId: 'agent-full-access' });
    stage = 'recovered prompt';
    await bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Verify recovery.' }] });
    assert.equal(modelCalls, callsBeforeLoad + 1);
    assert.equal(sha(fs.readFileSync(dirtyPath)), receipts.before.dirtySha256);
    stage = 'second graceful shutdown';
    receipts.secondShutdown = await bridge.stopBridgeOnly();
    const finalHistory = history(home, sessionId);
    assert(finalHistory.bytes.includes('Recovered same conversation.'));
    assert(finalHistory.bytes.subarray(0, firstHistory.bytes.length).equals(firstHistory.bytes), 'original transcript bytes must remain an exact prefix');
    receipts.afterReload = { sessionId, modelCalls, historySha256: sha(finalHistory.bytes), historyBytes: finalHistory.bytes.length, oldHistoryExactPrefix: true, dirtySha256: sha(fs.readFileSync(dirtyPath)) };
    fs.writeFileSync(join(directory, 'receipt.json'), JSON.stringify(receipts, null, 2));
    console.log(JSON.stringify({ nativeSha256: receipts.binarySha256, bridgeSha256: receipts.bridgeSha256,
      firstShutdown: receipts.firstShutdown, secondShutdown: receipts.secondShutdown,
      originalHistoryExactPrefix: receipts.afterReload.oldHistoryExactPrefix, shellGone: receipts.afterShutdown.toolGone }));
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    if (failure) {
      const tail = value => String(value).slice(-6000);
      const read = path => exists(path) ? tail(fs.readFileSync(path, 'utf8')) : null;
      const diagnostics = { stage, phase, directory, modelCalls, error: failure.stack,
        serverError: serverError?.stack, toolPid: read(toolPidPath), dirtyFileExists: exists(dirtyPath),
        bridges: bridges.map(current => ({ phase: current.phase, bridgePid: current.child.pid,
          bridgeAlive: alive(current.child.pid), nativePid: read(current.native.pidPath),
          nativeStatus: read(current.native.statusPath), nativeStderr: read(current.native.stderrPath),
          bridgeStderr: read(join(directory, current.phase + '-bridge-stderr.log')),
          sentFrames: tail(JSON.stringify(current.sentFrames)), receivedFrames: tail(JSON.stringify(current.frames)) })),
        // The next model request carries shell failure/output even when no PID
        // file was created. Include it in CI logs as well as retained evidence.
        lastModelInput: tail(JSON.stringify(requests.at(-1)?.input)) };
      fs.writeFileSync(join(directory, 'failure.json'), JSON.stringify(diagnostics, null, 2));
      console.error('Native EOF fixture failure: ' + JSON.stringify(diagnostics));
    }
    if (bridge && alive(bridge.child.pid)) { await bridge.stopBridgeOnly().catch(error => { console.error(error); }); }
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    if (failure || process.env.CMA_EOF_EVIDENCE_ROOT) {
      fs.writeFileSync(join(directory, 'model-requests.json'), JSON.stringify(requests, null, 2));
      for (const current of bridges) {
        fs.writeFileSync(join(directory, current.phase + '-acp-frames.json'), JSON.stringify(current.frames, null, 2));
        fs.writeFileSync(join(directory, current.phase + '-acp-sent-frames.json'), JSON.stringify(current.sentFrames, null, 2));
      }
    }
    else fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('cold load preserves a crashed previous process turn without restarting it or inventing a terminal event', { timeout: 180000 }, async () => {
  assert.match(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  assertReviewedNative();
  assert(['113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4',
    '4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888',
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c',
    'ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66',
    'fc9f74358c6ce5aeda4ac164c1daf83c513fb0f18df4b75f3d20bd0816fc6fa7',
    '8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4',
    '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4',
    'ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1',
    'e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484'].includes(sha(fs.readFileSync(bridgeProgram))));
  const directory = fs.mkdtempSync(join(evidence, 'cma-preboot-history-'));
  const home = join(directory, 'home'); fs.mkdirSync(home);
  let modelCalls = 0, bridge;
  const server = createServer(async (request, response) => {
    for await (const _ of request) { /* Consume only the isolated fixture request. */ }
    modelCalls++;
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    if (modelCalls === 1) {
      response.write('data: ' + JSON.stringify({ type: 'response.created', response: {
        id: 'resp_preboot_wait', status: 'in_progress', output: [],
      } }) + '\n\n');
    } else complete(response, 'resp_preboot_recovered', [message('msg_preboot_recovered', 'Original conversation recovered.')]);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  fs.writeFileSync(join(home, 'config.toml'), [
    'model="gpt-5.6-sol"', 'model_provider="cma_preboot_fixture"', 'web_search="disabled"',
    'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.cma_preboot_fixture]', 'name="Offline previous process fixture"',
    'base_url="http://127.0.0.1:' + server.address().port + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
  ].join('\n'));
  const turns = h => h.records.filter(r => r.type === 'event_msg' && ['task_started', 'turn_started',
    'task_complete', 'turn_complete', 'turn_aborted'].includes(r.payload.type));
  try {
    bridge = connect(directory, home, 'crashed');
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    const pending = bridge.call('session/prompt', { sessionId,
      prompt: [{ type: 'text', text: 'Wait for the isolated fixture response.' }] });
    void pending.catch(() => {});
    await until(() => modelCalls === 1 && turns(history(home, sessionId)).some(r => r.payload.type === 'task_started'),
      'real native unmatched start is persisted');
    const nativePid = Number(fs.readFileSync(bridge.native.pidPath, 'utf8'));
    // Only this disposable fixture is crashed to create genuine old history.
    // Production recovery never sends a signal to native or fabricates records.
    process.kill(nativePid, 'SIGKILL');
    await until(() => exists(bridge.native.statusPath), 'fixture native crash receipt');
    assert.equal(JSON.parse(fs.readFileSync(bridge.native.statusPath, 'utf8')).exitCode, -9);
    if (alive(bridge.child.pid)) process.kill(bridge.child.pid, 'SIGTERM');
    await bridge.exited; await pending.catch(() => {});
    const original = history(home, sessionId);
    const retainsOriginalWithoutNewTurns = (current, label) => {
      assert(current.bytes.subarray(0, original.bytes.length).equals(original.bytes), label + ': original bytes changed');
      // CLI 0.165 records settings on resume. Those metadata events are not
      // user/assistant messages, started turns, or invented terminal events.
      const added = current.records.slice(original.records.length);
      assert(added.every(record => record.type === 'event_msg' && record.payload.type === 'thread_settings_applied'
        && record.payload.thread_id === sessionId), label + ': unexpected appended records');
      assert.deepEqual(turns(current), turns(original), label + ': turn history changed');
    };
    const started = turns(original).find(r => r.payload.type === 'task_started');
    assert.equal(turns(original).length, 1, 'crash leaves an actual unmatched start');
    const startAt = Date.parse(started.timestamp);
    assert(Number.isFinite(startAt));
    await until(() => Date.now() - startAt > 61000, 'admitted preboot clock margin', 65000);
    const coldLaunchLowerBound = Date.now();
    bridge = connect(directory, home, 'cold');
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    await bridge.call('session/load', { sessionId, cwd: directory, mcpServers: [] });
    assert.equal(modelCalls, 1, 'loading an unmatched previous-process turn does not resume model work');
    retainsOriginalWithoutNewTurns(history(home, sessionId), 'cold load');
    const shutdown = await bridge.stopBridgeOnly();
    const drained = history(home, sessionId);
    retainsOriginalWithoutNewTurns(drained, 'idle native EOF');
    assert(!alive(Number(fs.readFileSync(bridge.native.pidPath, 'utf8'))));
    bridge = connect(directory, home, 'continued');
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    await bridge.call('session/load', { sessionId, cwd: directory, mcpServers: [] });
    assert.equal(modelCalls, 1);
    await bridge.call('session/prompt', { sessionId,
      prompt: [{ type: 'text', text: 'Continue the original fixture conversation.' }] });
    assert.equal(modelCalls, 2, 'only the new explicit prompt starts work');
    await bridge.stopBridgeOnly();
    const continued = history(home, sessionId);
    assert(continued.bytes.subarray(0, original.bytes.length).equals(original.bytes));
    assert(continued.records.some(r => JSON.stringify(r).includes('Original conversation recovered.')));
    const receipt = { directory, sessionId, turnId: started.payload.turn_id, startAt, coldLaunchLowerBound,
      prebootMarginMs: coldLaunchLowerBound - startAt, originalHistorySha256: sha(original.bytes),
      originalHistoryBytes: original.bytes.length, coldLoadStartedNoWork: true,
      drainedAddedNoTurns: true, originalHistoryExactPrefix: true, continuedOriginalPrefix: true, modelCalls, shutdown };
    fs.writeFileSync(join(directory, 'receipt.json'), JSON.stringify(receipt, null, 2));
    console.log(JSON.stringify(receipt));
  } finally {
    if (bridge && alive(bridge.child.pid)) await bridge.stopBridgeOnly();
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
});
