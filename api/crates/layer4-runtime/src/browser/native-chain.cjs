const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { createServer } = require('node:http');
const { readFile, readdir, rename, writeFile, unlink } = require('node:fs/promises');
const { join } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { runtimeFixture, conversation } = require('./native-http-fixture.cjs');

async function main() {
  const root = process.env.CMA_BROWSER_RELAY_DIRECTORY;
  assert(root, 'Explicit private proof relay directory required');
  const notifications = [];
  const navigated = [];
  let destination = 'a';
  let modelRound = 0;
  let modelError;
  const server = createServer(async (request, response) => {
    try {
      if (request.method === 'GET') {
        const page = request.url.slice(1);
        if (!['a', 'b'].includes(page)) { response.writeHead(404).end(); return; }
        navigated.push(page);
        response.setHeader('content-type', 'text/html');
        response.end(`<title>Page ${page}</title><body style="background:${page === 'a' ? '#fff0d4' : '#d4e4ff'}"><h1>PAGE ${page.toUpperCase()}</h1></body>`);
        return;
      }
      assert.equal(request.url, '/v1/responses');
      let body = '';
      for await (const bytes of request) { body += bytes; assert(body.length < 2 * 1024 * 1024); }
      const value = JSON.parse(body);
      const declared = value.tools ?? value.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const execution = declared.find(tool => tool.name === 'functions')?.tools?.find(tool => tool.name === 'exec');
      assert(execution || declared.some(tool => tool.name === 'exec_command'));
      const command = { cmd: '/usr/local/bin/cma-browser navigate ' + origin + '/' + destination, yield_time_ms: 10000, max_output_tokens: 1000 };
      const first = modelRound++ % 2 === 0;
      if (!first) assert(JSON.stringify(value.input.filter(item => ['function_call_output', 'custom_tool_call_output'].includes(item.type))).includes('navigated'));
      const output = first ? [{ id: 'fc_' + modelRound, call_id: 'call_' + modelRound, status: 'completed',
        ...(execution ? { type: 'custom_tool_call', name: 'exec', namespace: 'functions', input: 'text(await tools.exec_command(' + JSON.stringify(command) + '));' }
          : { type: 'function_call', name: 'exec_command', arguments: JSON.stringify(command) }) }]
        : [{ type: 'message', id: 'msg_' + modelRound, role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'Navigation completed through the Agent browser command.', annotations: [] }] }];
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = event => response.write('data: ' + JSON.stringify(event) + '\n\n');
      const identifier = 'resp_native_chain_' + modelRound;
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
  const atomic = async (name, value) => {
    await writeFile(join(root, name + '.tmp'), JSON.stringify(value));
    await rename(join(root, name + '.tmp'), join(root, name));
  };
  try {
    runtime = await runtimeFixture(origin, 'agt_shared_read', 'sbx_route');
    bridge = await conversation(runtime.origin, notifications, 'agt_shared_read');
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call('session/new', { cwd: runtime.cwd, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId: session.sessionId, modeId: 'agent-full-access' });
    await atomic('ready.json', { agent_id: 'agt_shared_read', sandbox_id: 'sbx_route', boot_id: runtime.boot,
      runtime_sha256: createHash('sha256').update(await readFile(runtime.binary)).digest('hex'),
      provider: '0.147.0', node: process.version });
    const deadline = Date.now() + 300000;
    let forwarded = 0;
    for (;;) {
      assert(Date.now() < deadline, 'Public browser proof deadline');
      const files = await readdir(root);
      if (files.includes('stop.json')) break;
      for (const file of files.filter(name => /^[a-f0-9-]+\.request\.json$/.test(name)).sort()) {
        const request = JSON.parse(await readFile(join(root, file), 'utf8'));
        let result;
        if (request.t === 'browser') {
          assert.equal(request.agent_id, 'agt_shared_read');
          assert.equal(request.sandbox_id, 'sbx_route');
          assert.equal(request.boot_id, runtime.boot);
          result = await runtime.control(request);
          forwarded += 1;
        } else {
          assert.equal(request.t, 'native_navigation');
          assert(['a', 'b'].includes(request.page));
          destination = request.page;
          await bridge.call('session/prompt', { sessionId: session.sessionId,
            prompt: [{ type: 'text', text: 'Use the mediated browser to navigate to local page ' + destination }] });
          if (modelError) throw modelError;
          assert(navigated.includes(destination));
          result = { page: destination, nativeToolCompletions: notifications.filter(value => value.params?.update?.sessionUpdate === 'tool_call_update' && value.params.update.status === 'completed').length };
        }
        await atomic(file.replace('.request.', '.response.'), result);
        await unlink(join(root, file));
      }
      await delay(40);
    }
    const completions = notifications.filter(value => value.params?.update?.sessionUpdate === 'tool_call_update' && value.params.update.status === 'completed').length;
    assert(completions >= 2);
    assert(navigated.includes('a') && navigated.includes('b'));
    await atomic('native-chain-result.json', { nativeToolCompletions: completions, modelCalls: modelRound,
      navigated, forwardedRuntimeRequests: forwarded, boot_id: runtime.boot,
      boundary: 'Actual native Agent and runtime HTTP; private file RPC replaces only network transport to the real public test handler' });
  } finally {
    bridge?.close();
    await runtime?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
