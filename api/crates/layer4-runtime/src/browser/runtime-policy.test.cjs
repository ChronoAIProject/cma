const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtemp, mkdir, readFile, rm, writeFile } = require('node:fs/promises');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
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

test('immutable runtime binary fences browser identity and survives production namespace denial without claiming frames', {
  timeout: 90000, skip: process.env.CMA_BROWSER_POLICY_PROOF !== '1',
}, async () => {
  assert.equal(process.platform, 'linux');
  const binary = process.env.CMA_RUNTIME_TEST_BINARY;
  const provider = process.env.CMA_CODEX_TEST_BINARY;
  const bridge = process.env.CMA_ACP_TEST_BRIDGE;
  assert(binary && provider && bridge);
  assert.equal(execFileSync(provider, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.147.0');
  assert.equal(createHash('sha256').update(await readFile(bridge)).digest('hex'),
    '84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c');
  const directory = await mkdtemp(join(tmpdir(), 'cma-browser-binary-'));
  const home = join(directory, 'home');
  await mkdir(home);
  const runtime = spawn(binary, [], {
    detached: true, stdio: ['ignore', 'pipe', 'pipe'],
    env: { PATH: process.env.PATH, TMPDIR: directory, CHRONO_RUNTIME_HOME: home,
      CHRONO_RUNTIME_STATE_ROOT: join(directory, 'state'), CHRONO_SANDBOX_ID: 'browser-proof',
      CHRONO_RUNTIME_GENERATION: '1', CHRONO_RUNTIME_BIND: '127.0.0.1:19415',
      CHRONO_RUNTIME_HOME_IDENTITY: 'a'.repeat(43) },
  });
  let diagnostic = '';
  runtime.stdout.on('data', bytes => { diagnostic += bytes; });
  runtime.stderr.on('data', bytes => { diagnostic += bytes; });
  const origin = 'http://127.0.0.1:19415';
  const control = async frame => {
    const response = await fetch(origin + '/runtime/control', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(frame),
      signal: AbortSignal.timeout(20000), redirect: 'error',
    });
    assert.equal(response.status, 200, diagnostic);
    return response.json();
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
    const status = await control({ t: 'runtime_status', sandbox_id: 'browser-proof', generation: 1,
      home_identity: 'a'.repeat(43) });
    assert.equal(status.t, 'runtime_status', JSON.stringify(status));
    assert.equal(status.agent_browser, true);
    const request = { t: 'browser', sandbox_id: 'browser-proof', runtime_generation: 1,
      boot_id: status.boot_id, agent_id: 'browser-agent', operation: 'status' };
    assert.equal((await control(request)).observation.state, 'not_enabled');
    assert.equal((await control({ ...request, boot_id: 'stale-boot' })).t, 'error');
    assert.equal((await control({ ...request, operation: 'enable' })).t, 'error');
    const workspace = await control({ t: 'prepare_workspace', operation_id: 'workspace:wks_browser:prepare:v1',
      workspace_id: 'wks_browser', name: 'browser-project', source: { type: 'init' }, clone_depth: 50,
      request_sha256: digest('chrono.workspace.prepare.v2', number(50), field('wks_browser'), field('browser-project'), Buffer.from([2])) });
    assert.equal(workspace.t, 'prepare_workspace', JSON.stringify(workspace));
    const worktree = await control({ t: 'add_worktree', authority_fence: 1, request: {
      operation_id: 'worktree:browser-project:browser-agent:add:v1', workspace: 'browser-project',
      name: 'browser-agent', base: workspace.resolved_commit,
      request_sha256: digest('chrono.worktree.add.v1', field('browser-project'), field('browser-agent'), field(workspace.resolved_commit)),
    } });
    assert.equal(worktree.t, 'add_worktree', JSON.stringify(worktree));
    assert.equal((await control({ t: 'materialize_agent_home', authority_fence: 1, request: {
      operation_id: 'agent:browser-agent:home:create:v1', agent_id: 'browser-agent', files: [], symlinks: [], skills: [],
      request_sha256: digest('chrono.agent.home.plan.v1', field('browser-agent'), number(0), number(0), number(0)),
    } })).t, 'materialize_agent_home');
    const agentHome = join(home, '.cma', 'agents', 'browser-agent', 'home');
    const cwd = worktree.receipt.path;
    const opened = await control({ t: 'open_agent', authority_fence: 1, request: {
      operation_id: 'agent:browser-agent:open:v1', agent_id: 'browser-agent', cwd, home: agentHome,
      program: process.execPath, args: [bridge], env: { CODEX_PATH: provider },
      request_sha256: digest('chrono.agent.launch.v1', field('browser-agent'), field(cwd), field(agentHome),
        field(process.execPath), number(1), field(bridge), number(1), field('CODEX_PATH'), field(provider)),
    } });
    assert.equal(opened.t, 'open_agent', JSON.stringify(opened));
    for (const operation of ['enable', 'status', 'observe']) {
      const result = await control({ ...request, operation, observer: 'binary-proof' });
      assert.equal(result.t, 'browser', JSON.stringify(result));
      assert.equal(result.agent_id, 'browser-agent');
      assert.equal(result.boot_id, status.boot_id);
      assert.equal(result.observation.state, 'unsupported');
      assert.equal(result.observation.context_id, null);
      assert.equal(result.observation.frame, null);
      assert.equal(result.observation.captures, 0);
      assert.equal(result.observation.observers, 0);
    }
    assert.equal((await control({ ...request, agent_id: 'other-agent' })).observation.state, 'not_enabled');
    assert.equal((await control({ ...request, operation: 'disable' })).observation.state, 'not_enabled');
    if (process.env.CMA_BROWSER_PROOF_DIRECTORY) {
      await writeFile(join(process.env.CMA_BROWSER_PROOF_DIRECTORY, 'runtime-policy.json'), JSON.stringify({
        platform: process.platform, node: process.version,
        runtimeSha256: createHash('sha256').update(await readFile(binary)).digest('hex'),
        browser: execFileSync('/usr/bin/chromium', ['--version'], { encoding: 'utf8' }).trim(),
        provider: '0.147.0', runtimeIdentityFenced: true, explicitEnablementRequired: true,
        namespaceDeniedState: 'unsupported', frames: 0, captures: 0, observers: 0,
        acceptance: 'Fail-closed contract only; actual native navigation and Linux live resource budget remain unverified',
      }, null, 2));
    }
  } finally {
    await stop(runtime);
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
});
