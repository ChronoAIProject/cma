// Private loopback dependencies for the opt-in scheduler/runtime continuity test.
// Runtime, Codex and ACP are actual executables. Model and provider discovery are fixtures.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { createServer as tcpServer } from 'node:net';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync, openSync, realpathSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { setTimeout as delay } from 'node:timers/promises';

const binary = process.env.CMA_CODEX_TEST_BINARY;
const bridge = process.env.CMA_ACP_TEST_BRIDGE;
assert.equal(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
assert.equal(createHash('sha256').update(readFileSync(bridge)).digest('hex'),
  'ed2ce5c01e53b0a991bab1fd6ccbd44d0182967b3eda44ab21538881be27dc4c');
const evidence = process.env.CMA_SUPERVISION_EVIDENCE_DIR ?? tmpdir();
mkdirSync(evidence, { recursive: true });
const root = mkdtempSync(join(realpathSync(evidence), 'runtime-'));
const home = join(root, 'home');
const stateRoot = join(root, 'state');
const transfer = join(root, 'transfer');
for (const path of [home, stateRoot, transfer]) mkdirSync(path);
const requests = [];
const responses = new Set();
let runtime;
let runtimePort;
let stopping = false;
let error;
let modelRelease;
const modelGate = new Promise(resolve => { modelRelease = resolve; });
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const file = name => quote(join(root, name));
const task = `printf 'effect\\n' >> ${file('effects.log')}; echo $$ > ${file('task.pid')}; touch ${file('started')}; while [ ! -f ${file('release-task')} ]; do sleep 0.1; done; printf 'completed\\n' >> ${file('task-result.txt')}; printf 'native-task-completed\\n'`;

function message(text, index) {
  return { id: 'msg_' + index, type: 'message', role: 'assistant', status: 'completed',
    content: [{ type: 'output_text', text, annotations: [] }] };
}

async function model(request, response) {
  let body = '';
  for await (const chunk of request) { body += chunk; assert(body.length < 8 * 1048576); }
  const value = JSON.parse(body);
  requests.push(value);
  writeFileSync(join(root, 'model-requests.json'), JSON.stringify(requests, null, 2));
  const index = requests.length;
  const declared = value.tools ?? value.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
  const execution = declared.find(tool => tool.name === 'functions')?.tools?.find(tool => tool.name === 'exec');
  let item;
  if (index === 1) {
    assert(execution || declared.some(tool => ['exec_command', 'shell_command', 'shell'].includes(tool.name)));
    const tool = execution ?? declared.find(tool => ['shell_command', 'shell', 'exec_command'].includes(tool.name));
    const args = tool.name === 'shell' ? { command: ['/bin/sh', '-c', task], timeout_ms: 120000 }
      : tool.name === 'shell_command' ? { command: task, timeout_ms: 120000 }
      : { cmd: task, yield_time_ms: 10000, max_output_tokens: 1000 };
    item = { id: 'fc_original', call_id: 'call_original', status: 'completed',
      ...(execution ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions',
        input: `let result = await tools.exec_command(${JSON.stringify(args)}); while (result.session_id) { result = await tools.write_stdin({session_id: result.session_id, chars: "", yield_time_ms: 1000, max_output_tokens: 1000}); } text(result);` }
        : { type: 'function_call', name: tool.name, arguments: JSON.stringify(args) }) };
  } else if (!existsSync(join(root, 'task-result.txt'))) {
    // The direct exec tool may return a live shell handle before the test releases it.
    const output = value.input.filter(item => ['function_call_output', 'custom_tool_call_output'].includes(item.type));
    const text = JSON.stringify(output);
    const session = text.match(/session ID[ :]+(\d+)/i);
    assert(session, 'expected a live shell handle, received: ' + text);
    item = { id: 'fc_poll_' + index, call_id: 'call_poll_' + index, status: 'completed', type: 'function_call',
      name: 'write_stdin', arguments: JSON.stringify({ session_id: Number(session[1]), chars: '', yield_time_ms: 1000 }) };
  } else {
    const followup = value.input.some(item => item.role === 'user' && JSON.stringify(item.content).includes('Continue the retained conversation'));
    if (!followup) {
      writeFileSync(join(root, 'thinking'), 'waiting for controlled model completion');
      await modelGate;
    } else {
      assert(JSON.stringify(value.input).includes('ORIGINAL_RUNTIME_RESULT'), 'continuation must retain the original answer');
    }
    item = message(followup ? 'CONTINUED_SAME_CONVERSATION' : 'ORIGINAL_RUNTIME_RESULT', index);
  }
  response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' });
  const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
  const id = 'resp_' + index;
  send({ type: 'response.created', response: { id, status: 'in_progress', output: [] } });
  send({ type: 'response.output_item.added', output_index: 0, item: { ...item, status: 'in_progress' } });
  if (item.type === 'message') {
    send({ type: 'response.content_part.added', item_id: item.id, output_index: 0, content_index: 0,
      part: { type: 'output_text', text: '', annotations: [] } });
    send({ type: 'response.output_text.delta', item_id: item.id, output_index: 0, content_index: 0,
      delta: item.content[0].text });
    send({ type: 'response.output_text.done', item_id: item.id, output_index: 0, content_index: 0,
      text: item.content[0].text });
    send({ type: 'response.content_part.done', item_id: item.id, output_index: 0, content_index: 0,
      part: item.content[0] });
  }
  send({ type: 'response.output_item.done', output_index: 0, item });
  send({ type: 'response.completed', response: { id, status: 'completed', output: [item],
    usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } } });
  response.end();
}

const server = createServer(async (request, response) => {
  responses.add(response);
  response.on('close', () => responses.delete(response));
  try {
    if (request.url === '/v1/responses') return await model(request, response);
    if (request.url.startsWith('/v1/sandboxes/')) {
      assert.equal(request.method, 'GET');
      assert.equal(request.headers['open-sandbox-api-key'], 'loopback-supervision-provider');
      assert.match(request.url, /^\/v1\/sandboxes\/runtime-proof\/endpoints\/9090\?use_server_proxy=true$/);
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ endpoint: '127.0.0.1:' + runtimePort }));
      return;
    }
    if (request.url === '/proof/release-task') writeFileSync(join(root, 'release-task'), 'release');
    else if (request.url === '/proof/release-model') modelRelease();
    else if (request.url === '/proof/state') {
      const status = { started: existsSync(join(root, 'started')), thinking: existsSync(join(root, 'thinking')),
        effects: existsSync(join(root, 'effects.log')) ? readFileSync(join(root, 'effects.log'), 'utf8').trim().split('\n').length : 0,
        task_result: existsSync(join(root, 'task-result.txt')) ? readFileSync(join(root, 'task-result.txt'), 'utf8') : null,
        task_pid: existsSync(join(root, 'task.pid')) ? Number(readFileSync(join(root, 'task.pid'), 'utf8')) : null,
        model_calls: requests.length, runtime_pid: runtime.pid, error: error?.message ?? null };
      status.task_alive = false;
      if (status.task_pid) { try { process.kill(status.task_pid, 0); status.task_alive = true; } catch {} }
      response.writeHead(200, { 'content-type': 'application/json' }); response.end(JSON.stringify(status)); return;
    } else if (request.url === '/proof/stop') {
      response.end('{}'); await close(); return;
    } else { response.writeHead(404).end(); return; }
    response.end('{}');
  } catch (failure) {
    error = failure;
    writeFileSync(join(root, 'fixture-error.txt'), failure.stack);
    if (!response.headersSent) response.writeHead(500);
    response.end('controlled fixture failed');
  }
});
process.on('SIGTERM', close);
process.on('SIGINT', close);
process.on('uncaughtException', failure => { error = failure; console.error(failure); void close(); });
process.on('unhandledRejection', failure => { error = failure; console.error(failure); void close(); });
process.stdin.resume();
process.stdin.on('end', close);
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = 'http://127.0.0.1:' + server.address().port;
const reservation = tcpServer();
await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve));
runtimePort = reservation.address().port;
await new Promise(resolve => reservation.close(resolve));
const log = openSync(join(root, 'runtime.log'), 'w', 0o600);
runtime = spawn(resolve(process.env.CMA_TEST_RUNTIME_BINARY), [], {
  env: { PATH: process.env.PATH, CHRONO_RUNTIME_HOME: home, CHRONO_RUNTIME_STATE_ROOT: stateRoot,
    CHRONO_RUNTIME_BIND: '127.0.0.1:' + runtimePort, CHRONO_SANDBOX_ID: 'sbx_commands',
    CHRONO_RUNTIME_GENERATION: '1', CHRONO_RUNTIME_HOME_IDENTITY: 'a'.repeat(43), TMPDIR: transfer,
    CODEX_PATH: binary }, stdio: ['ignore', log, log],
});
runtime.on('exit', (code, signal) => {
  writeFileSync(join(root, 'runtime-exit.json'), JSON.stringify({ code, signal }));
  if (!stopping) { error = new Error('runtime exited unexpectedly'); process.exitCode = 1; }
});
const deadline = Date.now() + 30000;
while (true) {
  assert(Date.now() < deadline, 'runtime readiness deadline');
  assert.equal(runtime.exitCode, null, 'runtime failed to start: ' + readFileSync(join(root, 'runtime.log'), 'utf8'));
  try { if ((await fetch('http://127.0.0.1:' + runtimePort + '/ready', { signal: AbortSignal.timeout(1000) })).ok) break; } catch {}
  await delay(50);
}
const manifest = { origin, root, home, runtime_pid: runtime.pid,
  runtime_origin: 'http://127.0.0.1:' + runtimePort, bridge, node: process.execPath,
  boundary: 'Real runtime/Codex/ACP and production transport; loopback model/provider discovery.' };
writeFileSync(join(root, 'manifest.json'), JSON.stringify(manifest, null, 2));
process.stdout.write(JSON.stringify(manifest) + '\n');

async function close() {
  if (stopping) return;
  stopping = true;
  modelRelease();
  writeFileSync(join(root, 'release-task'), 'cleanup');
  // Close the owned native agent even when the Rust assertion path exits early.
  // The digest uses the same length-prefixed wire contract as AgentCloseRequest.
  if (runtimePort && runtime?.exitCode === null) {
    const agent = 'agt_commands';
    const length = Buffer.alloc(8); length.writeBigUInt64BE(BigInt(Buffer.byteLength(agent)));
    const digest = createHash('sha256').update('chrono.agent.close.request.v1\0').update(length).update(agent).digest('hex');
    try {
      await fetch('http://127.0.0.1:' + runtimePort + '/runtime/control', {
        method: 'POST', headers: { 'content-type': 'application/json' }, signal: AbortSignal.timeout(3000),
        body: JSON.stringify({ t: 'close_agent', authority_fence: 1, request: {
          operation_id: 'agent:agt_commands:close:v1', request_sha256: digest, agent_id: agent } }),
      });
    } catch {}
  }
  runtime?.kill('SIGTERM');
  const end = Date.now() + 5000;
  while (runtime && runtime.exitCode === null && runtime.signalCode === null && Date.now() < end) await delay(50);
  if (runtime && runtime.exitCode === null && runtime.signalCode === null) runtime.kill('SIGKILL');
  for (const response of responses) response.destroy();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
  process.exit(error ? 1 : 0);
}
