// Execute Codex's existing shell/image tools on the same display streamed to users.
// Model responses are a loopback fixture; this check makes no inference request.
const assert = require('node:assert/strict');
const { createServer } = require('node:http');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync } = require('node:fs');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { setTimeout: delay } = require('node:timers/promises');
const { processRpc } = require('/opt/codex-fixtures/native_tools_fixture.cjs');

const directory = mkdtempSync('/tmp/cma-desktop-proof-');
const home = directory + '/home';
mkdirSync(home);
const screenshot = directory + '/screen.png';
const calls = [];
let failure;
let command;
function output(response, index, item) {
  response.writeHead(200, { 'content-type': 'text/event-stream' });
  const id = 'resp_desktop_' + index;
  for (const event of [
    { type: 'response.created', response: { id, status: 'in_progress', output: [] } },
    { type: 'response.output_item.added', output_index: 0, item },
    { type: 'response.output_item.done', output_index: 0, item },
    { type: 'response.completed', response: { id, status: 'completed', output: [item], usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } },
  ]) response.write('data: ' + JSON.stringify(event) + '\n\n');
  response.end();
}
function call(value, index, name, args) {
  const declared = value.tools ?? value.input.filter(i => i.type === 'additional_tools').flatMap(i => i.tools);
  const exec = declared.find(t => t.name === 'functions')?.tools?.find(t => t.name === 'exec');
  if (exec) return { type: 'custom_tool_call', id: 'fc_' + index, call_id: 'call_' + index,
    name: 'exec', namespace: 'functions', input: (name === 'view_image' ? 'image' : 'text') + '(await tools.' + name + '(' + JSON.stringify(args) + '));', status: 'completed' };
  assert(declared.some(t => t.name === name), 'missing built-in tool: ' + name);
  return { type: 'function_call', id: 'fc_' + index, call_id: 'call_' + index, name, arguments: JSON.stringify(args), status: 'completed' };
}
const server = createServer(async (request, response) => {
  try {
    if (request.method === 'GET') {
      response.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' });
      response.end('<!doctype html><title>cma-desktop-ready</title><body style="margin:0;background:#234"><input id="field" style="position:absolute;left:80px;top:100px;width:300px;height:40px"><button style="position:absolute;left:80px;top:200px;width:250px;height:40px" onclick="document.title=\'cma-ui:\'+document.getElementById(\'field\').value">Verify input</button>');
      return;
    }
    assert.equal(request.url, '/v1/responses');
    let body = '';
    for await (const chunk of request) { body += chunk; assert(body.length < 16 * 1024 * 1024); }
    const value = JSON.parse(body);
    calls.push(value);
    const index = calls.length;
    let item;
    if (index === 1) item = call(value, index, 'exec_command', { cmd: command, yield_time_ms: 10000, max_output_tokens: 2000 });
    else if (index === 2) {
      // The command fits within one native tool wait; prove its screen interaction
      // before asking the model to view the resulting image.
      assert.equal(execFileSync('xdotool', ['getactivewindow', 'getwindowname'], { encoding: 'utf8' }).trim().startsWith('cma-ui:cma-codex'), true);
      item = call(value, index, 'view_image', { path: screenshot });
    } else {
      assert.equal(index, 3);
      assert(JSON.stringify(value.input).includes('data:image/'), 'screenshot was not returned to the model');
      item = { type: 'message', id: 'msg_desktop', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'Desktop fixture passed.', annotations: [] }] };
    }
    output(response, index, item);
  } catch (error) {
    failure = error;
    if (!response.headersSent) response.writeHead(500);
    response.end();
  }
});
(async () => {
  assert.equal(process.getuid(), 10001);
  assert.equal(process.env.DISPLAY, ':99');
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  writeFileSync(home + '/config.toml', [
    'model="gpt-6"', 'model_provider="fixture"', 'web_search="disabled"',
    'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.fixture]', 'name="Offline desktop acceptance"',
    'base_url="' + origin + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
  ].join('\n'));
  const window = execFileSync('xdotool', ['search', '--onlyvisible', '--class', 'chromium'], { encoding: 'utf8' }).trim().split('\n')[0];
  assert(/^\d+$/.test(window));
  command = [
    'export DISPLAY=:99',
    'xdotool windowactivate --sync ' + window,
    'xdotool key --clearmodifiers ctrl+l',
    "xdotool type --clearmodifiers '" + origin + "/desktop-test'",
    'xdotool key Return',
    "for i in $(seq 1 30); do xdotool getactivewindow getwindowname | grep -q cma-desktop-ready && break; sleep 0.1; done",
    'xdotool key F11', 'sleep 0.3',
    'xdotool mousemove 120 120 click 1', 'xdotool type --clearmodifiers cma-codex',
    'xdotool mousemove 120 220 click 1', 'sleep 0.1',
    'scrot ' + screenshot,
    'xdotool getactivewindow getwindowname',
  ].join('\n');
  const bridge = processRpc(process.execPath, ['/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js'], {
    cwd: directory, env: { PATH: process.env.PATH, DISPLAY: ':99', CODEX_HOME: home, CODEX_PATH: '/usr/local/bin/codex' },
  });
  try {
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId, modeId: 'agent-full-access' });
    await bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Use the desktop, enter cma-codex, click Verify input, then inspect a screenshot.' }] });
    if (failure) throw failure;
    assert.equal(calls.length, 3);
    const bytes = readFileSync(screenshot);
    assert.equal(bytes.subarray(1, 4).toString(), 'PNG');
    assert(bytes.length > 1000);
    // A user takes over the very same browser through the authenticated stream.
    const child = require('node:child_process').spawn('python3', ['/opt/desktop-vnc-probe.py'], { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
    let logs = '';
    child.stdout.on('data', bytes => { logs += bytes; });
    child.stderr.on('data', bytes => { logs += bytes; });
    const code = await new Promise(resolve => child.once('exit', resolve));
    assert.equal(code, 0, logs);
    let title;
    for (let i = 0; i < 30; i++) {
      title = execFileSync('xdotool', ['getactivewindow', 'getwindowname'], { encoding: 'utf8' });
      if (title.startsWith('cma-ui:cma-user')) break;
      await delay(100);
    }
    assert(title.startsWith('cma-ui:cma-user'), 'user input did not reach the same browser');
    console.log(JSON.stringify({ checks: ['codex-shell-click-type', 'codex-view-image', 'authenticated-framebuffer', 'shared-browser-user-input'], screenshotSha256: createHash('sha256').update(bytes).digest('hex') }));
  } finally {
    await bridge.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; server.closeAllConnections(); server.close(); });
