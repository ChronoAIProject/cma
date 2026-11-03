const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtemp, mkdir, readFile, rm } = require('node:fs/promises');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { stop } = require('../../../layer3-adapter/src/codex/native_tools_fixture.cjs');

function number(value, width = 4) {
  const bytes = Buffer.alloc(width);
  if (width === 8) bytes.writeBigUInt64BE(BigInt(value));
  else bytes.writeUInt32BE(value);
  return bytes;
}
function field(value) {
  const bytes = Buffer.from(value);
  return Buffer.concat([number(bytes.length), bytes]);
}
function digest(prefix, ...values) {
  return createHash('sha256').update(Buffer.concat([Buffer.from(prefix + '\0'), ...values])).digest('hex');
}

async function runtimeFixture(modelOrigin, agentId = 'browser-agent', sandboxId = 'browser-proof') {
  const binary = process.env.CMA_RUNTIME_TEST_BINARY;
  const provider = process.env.CMA_CODEX_TEST_BINARY;
  const bridge = process.env.CMA_ACP_TEST_BRIDGE;
  assert(binary && provider && bridge, 'Explicit pinned runtime, provider and bridge required');
  assert.equal(execFileSync(provider, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
  assert.equal(createHash('sha256').update(await readFile(bridge)).digest('hex'),
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c');
  const directory = await mkdtemp(join(tmpdir(), 'cma-browser-http-native-'));
  const home = join(directory, 'home');
  await mkdir(home);
  const child = spawn(binary, [], {
    detached: true, stdio: ['ignore', 'pipe', 'pipe'],
    env: { PATH: process.env.PATH, TMPDIR: directory, CHRONO_RUNTIME_HOME: home,
      CHRONO_RUNTIME_STATE_ROOT: join(directory, 'state'), CHRONO_SANDBOX_ID: sandboxId,
      CHRONO_RUNTIME_GENERATION: '1', CHRONO_RUNTIME_BIND: '127.0.0.1:19415',
      CHRONO_RUNTIME_HOME_IDENTITY: 'a'.repeat(43) },
  });
  let diagnostic = '';
  child.stdout.on('data', bytes => { diagnostic = (diagnostic + bytes).slice(-16000); });
  child.stderr.on('data', bytes => { diagnostic = (diagnostic + bytes).slice(-16000); });
  const origin = 'http://127.0.0.1:19415';
  const control = async frame => {
    const response = await fetch(origin + '/runtime/control', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(frame),
      signal: AbortSignal.timeout(20000), redirect: 'error',
    });
    assert.equal(response.status, 200, diagnostic);
    return response.json();
  };
  const close = async () => {
    await stop(child);
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  };
  try {
    for (let attempt = 0; ; attempt += 1) {
      try { assert((await fetch(origin + '/ready')).ok); break; }
      catch { assert(attempt < 60, diagnostic); await delay(100); }
    }
    assert.equal((await control({ t: 'install', operation_id: 'browser-install', commands: [],
      timeout_s: 30, total_timeout_s: 60, output_limit: 4096,
      request_sha256: digest('chrono.install.request.v2', number(30, 8), number(60, 8), number(4096), number(0)) })).t, 'install');
    assert.equal((await control({ t: 'credential', operation_id: 'browser-credential',
      value: 'local-browser-proof', custody_generation: 1 })).installed, true);
    const status = await control({ t: 'runtime_status', sandbox_id: sandboxId, generation: 1,
      home_identity: 'a'.repeat(43) });
    assert.equal(status.agent_browser, true, JSON.stringify(status));
    const workspace = await control({ t: 'prepare_workspace', operation_id: 'workspace:wks_browser:prepare:v1',
      workspace_id: 'wks_browser', name: 'browser-project', source: { type: 'init' }, clone_depth: 50,
      request_sha256: digest('chrono.workspace.prepare.v2', number(50), field('wks_browser'), field('browser-project'), Buffer.from([2])) });
    assert.equal(workspace.t, 'prepare_workspace', JSON.stringify(workspace));
    const worktree = await control({ t: 'add_worktree', authority_fence: 1, request: {
      operation_id: 'worktree:browser-project:' + agentId + ':add:v1', workspace: 'browser-project',
      name: agentId, base: workspace.resolved_commit,
      request_sha256: digest('chrono.worktree.add.v1', field('browser-project'), field(agentId), field(workspace.resolved_commit)),
    } });
    assert.equal(worktree.t, 'add_worktree', JSON.stringify(worktree));
    const config = ['model = "gpt-5.6-sol"', 'model_provider = "browser_test"', 'web_search = "disabled"',
      'approval_policy = "never"', 'sandbox_mode = "danger-full-access"', '[model_providers.browser_test]',
      'name = "Local browser proof"', `base_url = "${modelOrigin}/v1"`, 'wire_api = "responses"',
      'requires_openai_auth = false'].join('\n');
    assert.equal((await control({ t: 'materialize_agent_home', authority_fence: 1, request: {
      operation_id: 'agent:' + agentId + ':home:create:v1', agent_id: agentId,
      files: [{ path: 'config.toml', bytes: [...Buffer.from(config)] }], symlinks: [], skills: [],
      request_sha256: digest('chrono.agent.home.plan.v1', field(agentId), number(1),
        field('config.toml'), field(config), number(0), number(0)),
    } })).t, 'materialize_agent_home');
    const agentHome = join(home, '.cma', 'agents', agentId, 'home');
    const cwd = worktree.receipt.path;
    const environment = { CODEX_HOME: agentHome, CODEX_PATH: provider };
    const opened = await control({ t: 'open_agent', authority_fence: 1, request: {
      operation_id: 'agent:' + agentId + ':open:v1', agent_id: agentId, cwd, home: agentHome,
      program: process.execPath, args: [bridge], env: environment,
      request_sha256: digest('chrono.agent.launch.v1', field(agentId), field(cwd), field(agentHome),
        field(process.execPath), number(1), field(bridge), number(2),
        ...Object.entries(environment).flatMap(([name, value]) => [field(name), field(value)])),
    } });
    assert.equal(opened.t, 'open_agent', JSON.stringify(opened));
    return { control, close, origin, cwd, binary, boot: status.boot_id,
      browser: async (operation, parameters = {}) => {
        const result = await control({ t: 'browser', sandbox_id: sandboxId, runtime_generation: 1,
          boot_id: status.boot_id, agent_id: agentId, operation, ...parameters });
        assert.equal(result.t, 'browser', JSON.stringify(result));
        assert.equal(result.agent_id, agentId);
        assert.equal(result.boot_id, status.boot_id);
        return result.observation;
      } };
  } catch (error) { await close(); throw error; }
}

async function conversation(origin, notifications, agentId = 'browser-agent') {
  const socket = new WebSocket(origin.replace('http:', 'ws:') + '/runtime/agents/' + agentId + '/conversation');
  socket.binaryType = 'arraybuffer';
  const pending = new Map();
  let identifier = 0;
  let readyResolve;
  const ready = new Promise(resolve => { readyResolve = resolve; });
  socket.addEventListener('message', event => {
    if (typeof event.data === 'string') { readyResolve(JSON.parse(event.data)); return; }
    const bytes = Buffer.from(event.data);
    assert.equal(bytes[0], 2);
    const frame = JSON.parse(bytes.subarray(1).toString());
    if (Object.hasOwn(frame, 'id')) pending.get(frame.id)?.(frame);
    else notifications.push(frame);
  });
  let readyDeadline;
  try {
    const announced = await Promise.race([ready, new Promise((resolve, reject) => {
      readyDeadline = setTimeout(() => reject(new Error('Runtime conversation readiness deadline')), 10000);
    })]);
    assert.equal(announced.agentId, agentId);
  } finally { clearTimeout(readyDeadline); }
  return {
    async call(method, params) {
      const current = ++identifier;
      const response = new Promise(resolve => pending.set(current, resolve));
      socket.send(Buffer.concat([Buffer.from([2]), Buffer.from(JSON.stringify({ jsonrpc: '2.0', id: current, method, params }))]));
      let deadline;
      try {
        const frame = await Promise.race([response, new Promise((resolve, reject) => {
          deadline = setTimeout(() => reject(new Error('Native runtime HTTP deadline: ' + method)), 30000);
        })]);
        assert.equal(frame.error, undefined, JSON.stringify(frame.error));
        return frame.result;
      } finally { clearTimeout(deadline); pending.delete(current); }
    },
    close() { socket.close(); },
  };
}

module.exports = { runtimeFixture, conversation };
