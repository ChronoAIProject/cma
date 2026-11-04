const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { readFile, readdir, rename, unlink, writeFile } = require('node:fs/promises');
const { createServer } = require('node:http');
const { join } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { runtimeFixture, conversation } = require('./native-http-fixture.cjs');

async function main() {
  const root = process.env.CMA_BROWSER_RELAY_DIRECTORY;
  const source = process.env.CMA_CONTROL_SOURCE_COMMIT;
  assert(root && /^[a-f0-9]{40}$/.test(source), 'Explicit private relay and exact source commit required');
  const values = [];
  const notifications = [];
  let pages = 0;
  let modelRound = 0;
  let nativeCommand;
  let modelFailure;
  const server = createServer(async (request, response) => {
    try {
      if (request.url === '/form') {
        pages += 1;
        response.writeHead(200, { 'content-type': 'text/html' });
        response.end('<body style="margin:20px"><h1>CONTROL PROOF</h1><input aria-label="Proof field" style="position:absolute;left:20px;top:70px;width:500px;height:40px" oninput="fetch(\'/value\',{method:\'POST\',body:this.value})"></body>');
        return;
      }
      if (request.url === '/favicon.ico') { response.writeHead(404).end(); return; }
      let body = '';
      for await (const chunk of request) body += chunk;
      if (request.url === '/value') { values.push(body); response.end('ok'); return; }
      assert.equal(request.url, '/v1/responses');
      const input = JSON.parse(body);
      const declared = input.tools ?? input.input.filter(item => item.type === 'additional_tools').flatMap(item => item.tools);
      const tool = declared.flatMap(tool => tool.tools ?? [tool]).find(tool => ['exec_command', 'shell_command', 'shell', 'exec'].includes(tool.name));
      assert(tool, 'Native execution tool required');
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
      const identifier = 'resp_control_chain_' + modelRound;
      send({ type: 'response.created', response: { id: identifier, status: 'in_progress', output: [] } });
      for (const [index, item] of output.entries()) {
        send({ type: 'response.output_item.added', output_index: index, item });
        send({ type: 'response.output_item.done', output_index: index, item });
      }
      send({ type: 'response.completed', response: { id: identifier, status: 'completed', output,
        usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } } });
      response.end();
    } catch (error) { modelFailure = error; if (!response.headersSent) response.writeHead(500); response.end(); }
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
      provider: '0.147.0', node: process.version, control_protocol: 1, source_commit: source });
    const deadline = Date.now() + 480000;
    const commands = [];
    let forwarded = 0;
    const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
    for (;;) {
      assert(Date.now() < deadline, 'Public control proof deadline');
      const files = await readdir(root);
      if (files.includes('stop.json')) break;
      for (const file of files.filter(name => /^[a-f0-9-]+\.request\.json$/.test(name)).sort()) {
        const request = JSON.parse(await readFile(join(root, file), 'utf8'));
        let result;
        if (['browser', 'browser_control'].includes(request.t)) {
          assert.equal(request.agent_id, 'agt_shared_read');
          assert.equal(request.sandbox_id, 'sbx_route');
          assert.equal(request.boot_id, runtime.boot);
          result = await runtime.control(request);
          forwarded += 1;
        } else if (request.t === 'form_state') {
          result = { values: [...values], pages };
        } else {
          assert.equal(request.t, 'native_control');
          assert(['navigate', 'input'].includes(request.command));
          const argument = request.command === 'navigate' ? origin + '/form' : JSON.stringify(request.input);
          nativeCommand = `${quote(process.execPath)} ${quote(join(__dirname, 'command.cjs'))} ${request.command} ${quote(argument)}`;
          const start = notifications.length;
          await bridge.call('session/prompt', { sessionId: session.sessionId,
            prompt: [{ type: 'text', text: 'Execute the bounded browser command once.' }] });
          if (modelFailure) throw modelFailure;
          const updates = notifications.slice(start);
          assert(updates.some(value => value.params?.update?.sessionUpdate === 'tool_call_update'), 'Native tool must execute');
          const refused = JSON.stringify(updates).includes('browser_control_conflict');
          await delay(100);
          result = { refused, values: [...values], pages, native_updates: updates };
          commands.push({ command: request.command, refused, values: [...values], pages });
        }
        await atomic(file.replace('.request.', '.response.'), result);
        await unlink(join(root, file));
      }
      await delay(20);
    }
    assert(commands.some(command => command.refused), 'Actual native input must be refused');
    assert(commands.some(command => command.command === 'input' && !command.refused), 'Actual native input must resume');
    assert(values.some(value => value.includes('USER-17')), 'Real form must receive user input');
    await atomic('native-chain-result.json', { commands, values, pages, modelCalls: modelRound,
      forwardedRuntimeRequests: forwarded, boot_id: runtime.boot, source_commit: source,
      boundary: 'Actual native Agent and runtime HTTP; private file RPC replaces only private network transport to real public handlers' });
  } finally {
    bridge?.close();
    await runtime?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
