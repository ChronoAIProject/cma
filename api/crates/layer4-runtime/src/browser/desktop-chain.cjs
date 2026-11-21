const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { createServer } = require('node:http');
const { mkdir, readFile, readdir, rename, unlink, writeFile } = require('node:fs/promises');
const { join } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { stop } = require('../../../layer3-adapter/src/codex/native_tools_fixture.cjs');

async function main() {
  const root = process.env.CMA_DESKTOP_RELAY_DIRECTORY;
  const binary = process.env.CMA_RUNTIME_TEST_BINARY;
  assert(root && binary && process.env.CMA_CODEX_TEST_BINARY && process.env.CMA_ACP_TEST_BRIDGE);
  assert.equal(execFileSync(process.env.CMA_CODEX_TEST_BINARY, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
  assert.equal(createHash('sha256').update(await readFile(process.env.CMA_ACP_TEST_BRIDGE)).digest('hex'),
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c');
  const atomic = async (name, value) => {
    await writeFile(join(root, name + '.tmp'), JSON.stringify(value));
    await rename(join(root, name + '.tmp'), join(root, name));
  };
  let rounds = 0;
  let completions = 0;
  let modelError;
  const pages = [];
  const model = createServer(async (request, response) => {
    try {
      if (request.method === 'GET') {
        if (request.url === '/page') {
          pages.push(Date.now());
          response.writeHead(200, { 'content-type': 'text/html' }).end('<title>Created desktop</title><body style="background:#e3f2df"><h1>ORIGINAL TASK DESKTOP</h1></body>');
        } else response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ data: [{ id: 'gpt-5.6-sol', object: 'model' }] }));
        return;
      }
      let body = '';
      for await (const bytes of request) { body += bytes; assert(body.length < 2 * 1024 * 1024); }
      const value = JSON.parse(body);
      const declared = value.tools ?? value.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const execution = declared.find(tool => tool.name === 'functions')?.tools?.find(tool => tool.name === 'exec');
      assert(execution || declared.some(tool => tool.name === 'exec_command'));
      const first = rounds++ === 0;
      if (first) assert(JSON.stringify(value.input).includes('比较三个工具'));
      else {
        assert(JSON.stringify(value.input.filter(item => ['function_call_output', 'custom_tool_call_output'].includes(item.type))).includes('navigated'));
        completions += 1;
      }
      const command = { cmd: '/usr/local/bin/cma-browser navigate http://127.0.0.1:16416/page', yield_time_ms: 10000, max_output_tokens: 1000 };
      const output = first ? [{ id: 'fc_desktop', call_id: 'call_desktop', status: 'completed',
        ...(execution ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions', input: 'text(await tools.exec_command(' + JSON.stringify(command) + '));' }
          : { type: 'function_call', name: 'exec_command', arguments: JSON.stringify(command) }) }]
        : [{ type: 'message', id: 'msg_desktop', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'The original task navigated its prepared browser.', annotations: [] }] }];
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const identifier = 'resp_desktop_' + rounds;
      send({ type: 'response.created', response: { id: identifier, status: 'in_progress', output: [] } });
      for (const [index, item] of output.entries()) {
        send({ type: 'response.output_item.added', output_index: index, item });
        send({ type: 'response.output_item.done', output_index: index, item });
      }
      send({ type: 'response.completed', response: { id: identifier, status: 'completed', output, usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } } });
      response.end();
      await atomic('native-progress.json', { rounds, completions, navigations: pages.length });
    } catch (error) { modelError = error; console.error(error); if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise(resolve => model.listen(16416, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:19416';
  const sockets = new Map();
  let runtime;
  let provision;
  let controls = 0;
  let diagnostic = '';
  const handle = async request => {
    if (request.kind === 'provision') {
      if (provision) { assert.deepEqual(request.body, provision); return { ready: true }; }
      provision = request.body;
      assert.equal(provision.image.uri, process.env.CMA_DESKTOP_RUNTIME_IMAGE);
      await mkdir(provision.env.CHRONO_RUNTIME_HOME, { recursive: true });
      runtime = spawn(binary, [], { detached: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
        ...provision.env, PATH: process.env.PATH, TMPDIR: '/tmp', CHRONO_RUNTIME_BIND: '127.0.0.1:19416',
        CHRONO_RUNTIME_STATE_ROOT: '/tmp/desktop-runtime-state',
      } });
      const log = bytes => { diagnostic = (diagnostic + bytes).slice(-16000); };
      runtime.stdout.on('data', log); runtime.stderr.on('data', log);
      for (let attempt = 0; ; attempt += 1) {
        try { assert((await fetch(origin + '/ready')).ok); break; }
        catch { assert(attempt < 100, diagnostic); await delay(100); }
      }
      await atomic('provisioned.json', { sandbox_id: provision.env.CHRONO_SANDBOX_ID, image: provision.image.uri,
        runtime_generation: provision.env.CHRONO_RUNTIME_GENERATION, resource_limits: provision.resourceLimits,
        runtime_sha256: createHash('sha256').update(await readFile(binary)).digest('hex') });
      return { ready: true };
    }
    assert(runtime, 'formal provisioning must precede runtime access');
    if (request.kind === 'http') {
      const response = await fetch(origin + request.path, { method: request.method, headers: request.headers,
        body: request.body === undefined ? undefined : Buffer.from(request.body, 'base64'), signal: AbortSignal.timeout(60000), redirect: 'error' });
      const body = Buffer.from(await response.arrayBuffer());
      if (request.path === '/runtime/control') controls += 1;
      return { status: response.status, headers: Object.fromEntries(response.headers), body: body.toString('base64') };
    }
    if (request.kind === 'socket_open') {
      const socket = new WebSocket(origin.replace('http:', 'ws:') + request.path);
      socket.binaryType = 'arraybuffer';
      let sequence = 0;
      let writes = Promise.resolve();
      socket.addEventListener('message', event => {
        const binary = typeof event.data !== 'string';
        const packet = { binary, data: Buffer.from(event.data).toString('base64') };
        const name = request.id + '.' + String(++sequence).padStart(8, '0') + '.packet.json';
        writes = writes.then(() => atomic(name, packet));
      });
      await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }); });
      sockets.set(request.id, socket);
      return { ready: true };
    }
    if (request.kind === 'socket_send') {
      const data = Buffer.from(request.data, 'base64');
      sockets.get(request.id).send(request.binary ? data : data.toString());
      return {};
    }
    if (request.kind === 'socket_close') { sockets.get(request.id)?.close(); return {}; }
    throw new Error('Unknown proof transport request');
  };
  try {
    await atomic('native-ready.json', { provider: '0.147.0', node: process.version });
    const deadline = Date.now() + 300000;
    while (!(await readdir(root)).includes('stop.json')) {
      assert(Date.now() < deadline, 'Formal desktop proof deadline: ' + diagnostic);
      for (const file of (await readdir(root)).filter(name => /^[a-f0-9-]+\.request\.json$/.test(name)).sort()) {
        const request = JSON.parse(await readFile(join(root, file), 'utf8'));
        await unlink(join(root, file));
        let result;
        try { result = await handle(request); } catch (error) { console.error(error, diagnostic); result = { error: String(error) }; }
        await atomic(file.replace('.request.json', '.response.json'), result);
      }
      await delay(15);
    }
    assert.equal(modelError, undefined);
    assert.equal(rounds, 2);
    assert.equal(completions, 1);
    assert(pages.length >= 1);
    await atomic('native-result.json', { passed: true, controls, rounds, completions, navigations: pages.length,
      sandbox_id: provision.env.CHRONO_SANDBOX_ID, runtime_sha256: createHash('sha256').update(await readFile(binary)).digest('hex') });
  } finally {
    for (const socket of sockets.values()) socket.close();
    if (runtime) await stop(runtime);
    model.closeAllConnections();
    await new Promise(resolve => model.close(resolve));
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
