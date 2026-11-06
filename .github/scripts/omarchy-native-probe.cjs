// Execute Codex's existing shell/image tools on the same display streamed to users.
// Model responses are a loopback fixture; this check makes no inference request.
const assert = require('node:assert/strict');
const { createServer } = require('node:http');
const { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } = require('node:fs');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { setTimeout: delay } = require('node:timers/promises');
const { processRpc } = require('/opt/codex-fixtures/native_tools_fixture.cjs');

const directory = mkdtempSync('/tmp/cma-desktop-proof-');
const home = directory + '/home';
mkdirSync(home);
const screenshot = directory + '/screen.png';
const terminalScreenshot = '/tmp/cma-omarchy-terminal-proof.png';
const terminalMarker = directory + '/terminal-result';
const calls = [];
let failure;
let command;
let terminalCommand;
let retainedProfile;
const toolStates = new Map();
function captureFailureScreen() {
  const path = '/tmp/cma-desktop-failed-screen.png';
  try { if (!existsSync(path)) execFileSync('scrot', [path]); }
  catch { /* Preserve the original error when the display is unavailable. */ }
}
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
      if (request.url.startsWith('/profile?value=')) {
        retainedProfile = decodeURIComponent(request.url.slice('/profile?value='.length));
        response.writeHead(204).end();
        return;
      }
      response.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' });
      response.end('<!doctype html><title>cma-desktop-ready</title><body style="margin:0;background:#234"><input id="field" style="position:absolute;left:80px;top:100px;width:300px;height:40px"><button style="position:absolute;left:80px;top:200px;width:250px;height:40px" onclick="localStorage.setItem(\'cma-desktop-proof\',document.getElementById(\'field\').value);document.title=\'cma-ui:\'+document.getElementById(\'field\').value">Verify input</button><script>fetch(\'/profile?value=\'+encodeURIComponent(localStorage.getItem(\'cma-desktop-proof\')||\'\'))</script>');
      return;
    }
    assert.equal(request.url, '/v1/responses');
    let body = '';
    for await (const chunk of request) { body += chunk; assert(body.length < 16 * 1024 * 1024); }
    const value = JSON.parse(body);
    calls.push(value);
    const index = calls.length;
    let item;
    if (index === 1) item = call(value, index, 'exec_command', { cmd: terminalCommand, yield_time_ms: 30000, max_output_tokens: 2000 });
    else if (index === 2) {
      assert.equal(readFileSync(terminalMarker, 'utf8'), 'CMA terminal control verified\n');
      item = call(value, index, 'view_image', { path: terminalScreenshot });
    } else if (index === 3) {
      assert(JSON.stringify(value.input).includes('data:image/'), 'terminal screenshot was not returned to the model');
      item = call(value, index, 'exec_command', { cmd: command, yield_time_ms: 30000, max_output_tokens: 2000 });
    } else if (index === 4) {
      // The command fits within one native tool wait; prove its screen interaction
      // before asking the model to view the resulting image.
      const title = execFileSync('python3', ['/opt/probes/omarchy-active-title.py'], { encoding: 'utf8' }).trim();
      assert(title.startsWith('cma-ui:cma-codex'), 'desktop click/type did not update the browser title: ' + title);
      item = call(value, index, 'view_image', { path: screenshot });
    } else {
      assert.equal(index, 5);
      assert(JSON.stringify(value.input).includes('data:image/'), 'screenshot was not returned to the model');
      item = { type: 'message', id: 'msg_desktop', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'Desktop fixture passed.', annotations: [] }] };
    }
    output(response, index, item);
  } catch (error) {
    if (!failure) {
      failure = error;
      process.stderr.write(`Desktop model round ${calls.length}: ${error.message}\n`);
      for (const item of calls.at(-1)?.input ?? []) {
        if ((item.type === 'function_call_output' || item.type === 'custom_tool_call_output') && typeof item.output === 'string') {
          process.stderr.write(item.output.slice(0, 2000) + '\n');
        }
      }
      captureFailureScreen();
    }
    // A fixture assertion must finish the native turn and report its original
    // failure, rather than turning into repeated HTTP retries that hide it.
    if (!response.headersSent) output(response, calls.length, {
      type: 'message', id: 'msg_desktop_failed', role: 'assistant', status: 'completed',
      content: [{ type: 'output_text', text: 'Desktop fixture failed.', annotations: [] }],
    });
    else response.end();
  }
});
(async () => {
  assert.equal(process.getuid(), 10001);
  assert.equal(process.env.DISPLAY, ':99');
  assert.equal(execFileSync('/usr/local/bin/codex', ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.165.0');
  assert.match(execFileSync('/usr/local/bin/codex', ['features', 'list'], { encoding: 'utf8' }), /^computer_use\s+stable\s+true$/m);
  // A stable loopback origin proves real Chromium profile storage after restart.
  await new Promise(resolve => server.listen(8877, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  writeFileSync(home + '/config.toml', [
    'model="gpt-6"', 'model_provider="fixture"', 'web_search="disabled"',
    'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.fixture]', 'name="Offline desktop acceptance"',
    'base_url="' + origin + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
  ].join('\n'));
  const viewerPid = execFileSync('pgrep', ['-x', 'vncviewer'], { encoding: 'utf8' }).trim();
  assert(/^\d+$/.test(viewerPid));
  const window = execFileSync('xdotool', ['search', '--onlyvisible', '--pid', viewerPid], { encoding: 'utf8' }).trim().split('\n')[0];
  assert(/^\d+$/.test(window));
  terminalCommand = [
    'set -e', 'export DISPLAY=:99',
    'xdotool windowfocus --sync ' + window,
    'xdotool key --clearmodifiers ctrl+alt+t',
    "for i in $(seq 1 40); do python3 /opt/probes/omarchy-active-title.py --class | grep -qi foot && break; sleep 0.1; done",
    'python3 /opt/probes/omarchy-active-title.py --class | grep -qi foot',
    'sleep 0.3',
    // This command reaches the shell only through keyboard events in Foot.
    "xdotool type --clearmodifiers --delay 2 'echo \"CMA terminal control verified\" | tee " + terminalMarker + "'",
    'xdotool key Return',
    'for i in $(seq 1 30); do test -s ' + terminalMarker + ' && break; sleep 0.1; done',
    'test -s ' + terminalMarker, 'sleep 0.3',
    'scrot --overwrite ' + terminalScreenshot,
  ].join('\n');
  command = [
    'set -e',
    'export DISPLAY=:99',
    'xdotool windowfocus --sync ' + window,
    'xdotool key --clearmodifiers ctrl+d', 'sleep 0.3',
    // Close the native Wayland window, then require the real launcher to reopen
    // it and finish both native shell calls while the browser remains alive.
    'python3 /opt/probes/omarchy-close-browser.py --normal-window',
    '/usr/local/bin/cma-desktop-session --browser ' + origin + '/desktop-test',
    '/usr/local/bin/cma-desktop-session --browser',
    'xdotool key --clearmodifiers ctrl+l',
    "xdotool type --clearmodifiers '" + origin + "/desktop-test'",
    'xdotool key Return',
    "for i in $(seq 1 30); do python3 /opt/probes/omarchy-active-title.py | grep -q cma-desktop-ready && break; sleep 0.1; done",
    // Chromium's own full-screen command hides its browser chrome. Merely
    // changing the window-manager state leaves the page origin offset by it.
    'xdotool key --clearmodifiers F11', 'sleep 0.5',
    'xdotool mousemove 120 120 click 1', 'xdotool type --clearmodifiers cma-codex',
    'xdotool mousemove 120 220 click 1', 'sleep 0.1',
    'scrot ' + screenshot,
    'python3 /opt/probes/omarchy-active-title.py',
  ].join('\n');
  const bridge = processRpc(process.execPath, ['/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js'], {
    cwd: directory, env: { PATH: process.env.PATH, DISPLAY: ':99', CODEX_HOME: home, CODEX_PATH: '/usr/local/bin/codex' },
  }, frame => {
    const update = frame.params?.update;
    if (update?.toolCallId && update.status) toolStates.set(update.toolCallId, update.status);
  }, 60000);
  try {
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call('session/new', { cwd: directory, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId, modeId: 'agent-full-access' });
    const completed = await bridge.call('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Open a desktop terminal, run a command and inspect its screenshot. Then use the browser, enter cma-codex, click Verify input, and inspect a screenshot.' }] });
    if (failure) throw failure;
    assert.equal(completed.stopReason, 'end_turn');
    assert(toolStates.size > 0, 'native tool terminal evidence is missing');
    assert([...toolStates.values()].every(status => status === 'completed'), 'native tools remain active');
    assert.equal(calls.length, 5);
    for (let i = 0; retainedProfile === undefined && i < 30; i++) await delay(100);
    assert.equal(retainedProfile, process.env.CMA_DESKTOP_EXPECT_RETAINED === '1' ? 'cma-user' : '');
    const bytes = readFileSync(screenshot);
    assert.equal(bytes.subarray(1, 4).toString(), 'PNG');
    assert(bytes.length > 1000);
    assert(execFileSync('python3', ['/opt/probes/omarchy-active-title.py'], { encoding: 'utf8' }).startsWith('cma-ui:cma-codex'), 'page did not remain open after end_turn');
    // A user takes over the very same browser through the authenticated stream.
    const child = require('node:child_process').spawn('python3', ['/opt/probes/omarchy-vnc-probe.py'], { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
    let logs = '';
    child.stdout.on('data', bytes => { logs += bytes; });
    child.stderr.on('data', bytes => { logs += bytes; });
    const code = await new Promise(resolve => child.once('exit', resolve));
    assert.equal(code, 0, logs);
    let title;
    for (let i = 0; i < 30; i++) {
      title = execFileSync('python3', ['/opt/probes/omarchy-active-title.py'], { encoding: 'utf8' });
      if (title.startsWith('cma-ui:cma-user')) break;
      await delay(100);
    }
    assert(title.startsWith('cma-ui:cma-user'), 'user input did not reach the same browser: ' + title);
    console.log(JSON.stringify({ checks: ['browser-reopen-tool-completed', 'native-end-turn-with-page-open', 'codex-open-terminal', 'codex-type-run-terminal-command', 'codex-terminal-view-image', 'codex-shell-click-type', 'codex-view-image', 'authenticated-framebuffer', 'shared-browser-user-input'], screenshotSha256: createHash('sha256').update(bytes).digest('hex'), terminalScreenshotSha256: createHash('sha256').update(readFileSync(terminalScreenshot)).digest('hex') }));
  } finally {
    await bridge.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { captureFailureScreen(); console.error(error); process.exitCode = 1; server.closeAllConnections(); server.close(); });
