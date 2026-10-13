import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, lstatSync, readdirSync, readlinkSync, rmSync, symlinkSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { githubProxyProbe } from './runtime-github-probe.mjs';

export const checks = [
  'installed-artifacts', 'non-root-read-only', 'empty-volume-initialization', 'credential-before-install-refused',
  'install-exact-replay', 'credential-after-install', 'github-api-proxy', 'daemon-writer-active',
  'agent-home-helper-aliases', 'agent-shell-inherited-path', 'escaping-helper-refused',
  'checkpoint-writers-held', 'checkpoint-exact-replay', 'wrong-resume-token-refused',
  'mutation-while-held-refused', 'checkpoint-download-digest', 'writer-resumed',
  'checkpoint-local-release', 'retired-hold-refused', 'checkpoint-reimport',
  'whole-home-restore', 'restore-local-release',
];
const checkpointFailures = [
  'writer_fence_failed', 'checkpoint_io_failed', 'checkpoint_changed',
  'checkpoint_manifest_failed', 'checkpoint_too_large', 'checkpoint_task_failed',
  'checkpoint_store_invalid', 'checkpoint_collision', 'checkpoint_hold_retired',
  'checkpoint_hold_conflict', 'writer_resume_token_invalid',
  'checkpoint_home_invalid', 'checkpoint_path_invalid', 'checkpoint_symlink_escape',
  'checkpoint_symlink_invalid', 'checkpoint_entry_unsupported', 'checkpoint_invalid',
  'invalid_path', 'checkpoint_writer_state_invalid', 'checkpoint_writer_state_unavailable',
  'checkpoint_writers_held',
];
export const diagnosticStages = [
  'runtime-ready', ...checks, 'complete', 'checkpoint-request',
  'checkpoint-receipt-type', 'checkpoint-receipt-writers', 'checkpoint-receipt-token',
  'checkpoint-writer-stable',
  'checkpoint-refused-codex-temporary-symlink-escape',
  ...checkpointFailures.map((code) => `checkpoint-refused-${code.replaceAll('_', '-')}`),
];
export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

export async function waitForWriterProgress(read, { timeoutMs = 5000, intervalMs = 25 } = {}) {
  const deadline = performance.now() + timeoutMs;
  let first;
  while (performance.now() < deadline) {
    let value;
    try { value = read(); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (typeof value === 'string' && /^[1-9][0-9]*$/.test(value)) {
      const count = BigInt(value);
      if (first !== undefined && count > first) return value;
      first ??= count;
    }
    await delay(intervalMs);
  }
  assert.fail('background writer did not advance before the deadline');
}

function homeSnapshot(root) {
  const entries = [];
  const visit = (relative) => {
    const path = join(root, relative);
    const stat = lstatSync(path);
    const entry = { path: relative, mode: stat.mode };
    if (stat.isDirectory()) {
      entries.push({ ...entry, kind: 'directory' });
      for (const child of readdirSync(path).sort()) visit(join(relative, child));
    } else if (stat.isFile()) entries.push({ ...entry, kind: 'file', sha256: sha256(readFileSync(path)) });
    else if (stat.isSymbolicLink()) entries.push({ ...entry, kind: 'symlink', target: readlinkSync(path) });
    else assert.fail('sandbox home has an unsupported entry');
  };
  visit('');
  return sha256(JSON.stringify(entries));
}

function initializeEmptyVolume() {
  const volume = mkdtempSync(join(tmpdir(), 'cma-empty-home-'));
  const source = '/usr/share/cma-runtime-home';
  const initialize = () => execFileSync('/usr/local/bin/layer4-runtime', ['--initialize-home', volume], {
    encoding: 'utf8', timeout: 30000, stdio: ['ignore', 'pipe', 'pipe'],
  });
  const before = homeSnapshot('/home/cma');
  const publicHome = homeSnapshot(source);
  try {
    assert.equal(lstatSync(source).uid, 0);
    for (const path of ['.codex/auth.json', '.nyxid/access_token', '.cma/provider/codex/auth.json']) {
      assert.equal(existsSync(join(source, path)), false);
    }
    initialize();
    for (const name of ['home', 'state', 'transfer', '.cma-home-layout-v1']) {
      const metadata = lstatSync(join(volume, name));
      assert.ok(metadata.isDirectory() && !metadata.isSymbolicLink());
      assert.equal(metadata.uid, 10001);
    }
    for (const name of ['nyxid', 'ornn-agent-manual-cli']) {
      const relative = join('.codex', 'skills', name, 'SKILL.md');
      assert.deepEqual(readFileSync(join(volume, 'home', relative)), readFileSync(join(source, relative)));
    }
    assert.deepEqual(readdirSync(join(volume, 'state')), []);
    assert.deepEqual(readdirSync(join(volume, 'transfer')), []);
    writeFileSync(join(volume, 'home', 'uncommitted-work'), 'preserve across suspension');
    writeFileSync(join(volume, 'state', 'retained-binding'), 'same identity');
    symlinkSync('uncommitted-work', join(volume, 'home', 'user-link'));
    const retained = homeSnapshot(volume);
    initialize();
    assert.equal(homeSnapshot(volume), retained);
    assert.equal(homeSnapshot('/home/cma'), before);
    assert.equal(homeSnapshot(source), publicHome);
    rmSync(join(volume, '.cma-home-layout-v1'), { recursive: true });
    assert.throws(initialize);
    assert.equal(readFileSync(join(volume, 'home', 'uncommitted-work'), 'utf8'), 'preserve across suspension');
  } finally { rmSync(volume, { recursive: true, force: true }); }
}

export function installedCodexVersion(home, command = execFileSync) {
  const before = homeSnapshot(home);
  const scratch = mkdtempSync(join(tmpdir(), 'cma-codex-version-'));
  try {
    // Even --version creates executable-helper symlinks in CODEX_HOME.
    const version = command('codex', ['--version'], {
      encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, CODEX_HOME: scratch },
    }).trim();
    assert.equal(homeSnapshot(home), before);
    return version;
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}

export function assertAgentShellPath(agentHome, toolDirectory, command = execFileSync) {
  const executable = process.execPath;
  const inspect = 'console.log(JSON.stringify({home:process.env.HOME,path:process.env.PATH,tool:require("node:child_process").execFileSync("cma-shell-path-fixture",{encoding:"utf8"})}))';
  const shellCommand = `${executable} -e '${inspect}'`;
  for (const path of [`${toolDirectory}:${process.env.PATH}`, `${process.env.PATH}:${toolDirectory}`]) {
    for (const mode of ['-c', '-lc']) {
      const actual = JSON.parse(command('/bin/bash', [mode, shellCommand], {
        encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
        env: { ...process.env, HOME: agentHome, CODEX_HOME: agentHome, PATH: path },
      }));
      assert.deepEqual(actual, { home: agentHome, path, tool: 'cma-shell-path-ok\n' });
    }
  }
  const fallback = '/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games';
  for (const setup of ['PATH=""', 'unset PATH']) {
    const actual = command('/bin/bash', ['--noprofile', '--norc', '-c', `${setup}\n. /etc/profile\n/usr/bin/printf '%s' "$PATH"`], {
      encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, HOME: agentHome, CODEX_HOME: agentHome },
    });
    assert.equal(actual, fallback);
  }
}

export function assertCheckpointHoldReceipt(receipt, hold, onStage) {
  const code = checkpointFailures.find((value) => receipt?.t === 'failure' && receipt.code === value);
  onStage(code ? `checkpoint-refused-${code.replaceAll('_', '-')}` : 'checkpoint-receipt-type');
  if (code === 'checkpoint_symlink_escape' && /^checkpoint symlink \.codex\/tmp\/[^\r\n]+ escapes the sandbox home$/.test(receipt.detail ?? '')) {
    onStage('checkpoint-refused-codex-temporary-symlink-escape');
  }
  assert.equal(receipt.t, hold.t);
  onStage('checkpoint-receipt-writers');
  assert.equal(receipt.checkpoint.writers, 'held');
  onStage('checkpoint-receipt-token');
  assert.equal(receipt.writer_resume_token, hold.writer_resume_token);
}

export function installRequest(commands) {
  const parts = [Buffer.from('chrono.install.request.v2\0')];
  for (const value of [30, 60]) {
    const bytes = Buffer.alloc(8);
    bytes.writeBigUInt64BE(BigInt(value));
    parts.push(bytes);
  }
  for (const value of [4096, commands.length]) {
    const bytes = Buffer.alloc(4);
    bytes.writeUInt32BE(value);
    parts.push(bytes);
  }
  for (const command of commands) {
    const value = Buffer.from(command);
    const size = Buffer.alloc(4);
    size.writeUInt32BE(value.length);
    parts.push(size, value);
  }
  return {
    t: 'install', operation_id: 'ci-install', request_sha256: sha256(Buffer.concat(parts)),
    commands, timeout_s: 30, total_timeout_s: 60, output_limit: 4096,
  };
}

export async function probe(onStage = () => {}) {
  const completed = [];
  const complete = (check) => {
    assert.equal(check, checks[completed.length]);
    completed.push(check);
    onStage(checks[completed.length] ?? 'complete');
  };
  onStage('runtime-ready');
  const secret = randomBytes(32).toString('hex');
  const control = async (frame) => {
    const response = await fetch('http://127.0.0.1:9090/runtime/control', {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify(frame), signal: AbortSignal.timeout(60_000), redirect: 'error',
    });
    assert.equal(response.status, 200);
    const body = await response.text();
    assert.ok(body.length < 65536 && !body.includes(secret));
    return JSON.parse(body);
  };
  for (let attempt = 0; ; attempt += 1) {
    try {
      const response = await fetch('http://127.0.0.1:9090/ready', {
        signal: AbortSignal.timeout(1000), redirect: 'error',
      });
      assert.deepEqual(await response.json(), { status: 'ready', phase: 'provisioned' });
      break;
    } catch {
      assert.ok(attempt < 29, 'runtime did not become provisioned');
      await delay(1000);
    }
  }
  onStage(checks[0]);
  const artifacts = {};
  for (const [name, path] of Object.entries({
    runtime: '/usr/local/bin/layer4-runtime',
    gh: '/usr/local/bin/gh',
    launcher: '/usr/local/lib/node_modules/@openai/codex/bin/codex.js',
    native: '/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex',
    acp: '/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js',
  })) artifacts[name] = sha256(readFileSync(path));
  artifacts.codexVersion = installedCodexVersion('/home/cma');
  artifacts.acpVersion = JSON.parse(readFileSync('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json')).version;
  const ghVersion = execFileSync('/usr/local/bin/gh', ['--version'], { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] });
  artifacts.ghVersion = /^gh version ([0-9]+\.[0-9]+\.[0-9]+) /.exec(ghVersion)?.[1];
  complete('installed-artifacts');
  assert.equal(process.getuid(), 10001);
  assert.throws(() => writeFileSync('/usr/local/cma-ci-write-refusal', 'refused'));
  complete('non-root-read-only');
  initializeEmptyVolume();
  complete('empty-volume-initialization');
  const credential = { t: 'credential', operation_id: 'ci-credential', value: secret, custody_generation: 1 };
  assert.equal((await control(credential)).code, 'install_not_completed');
  complete('credential-before-install-refused');

  // The detached writer remains an adopted supervisor child after installation.
  const writer = [
    'import os,time', 'pid=os.fork()', 'if pid: os._exit(0)', 'os.setsid()',
    'pid=os.fork()', 'if pid: os._exit(0)', 'n=0',
    'while True:', ' n+=1',
    ' with open("/home/cma/ci-writer-next", "w") as f: f.write(str(n))',
    ' os.replace("/home/cma/ci-writer-next", "/home/cma/ci-writer")',
    ' time.sleep(0.02)',
  ].join('\n');
  const request = installRequest([
    'test "$(id -u)" = 10001 && test -z "${CHRONO_SANDBOX_ID+x}"',
    `python3 -c '${writer}' </dev/null >/dev/null 2>&1`,
  ]);
  const installed = await control(request);
  assert.equal(installed.t, 'install');
  assert.equal(installed.request_sha256, request.request_sha256);
  assert.equal(installed.commands.length, 2);
  assert.ok(installed.commands.every((command) => command.exit_code === 0));
  assert.deepEqual(await control(request), installed);
  complete('install-exact-replay');
  assert.deepEqual(await control(credential), { t: 'credential', operation_id: credential.operation_id, installed: true });
  complete('credential-after-install');
  await githubProxyProbe(secret);
  complete('github-api-proxy');
  const counter = () => readFileSync('/home/cma/ci-writer', 'utf8');
  await waitForWriterProgress(counter);
  complete('daemon-writer-active');
  const agentHome = '/home/cma/.cma/agents/ci-agent/home';
  const retained = [
    [`${agentHome}/sessions/ci-history`, 'retained-conversation-fixture'],
    [`${agentHome}/config.toml`, 'model = "retained-fixture"\n'],
    [`${agentHome}/skills/fixture/SKILL.md`, 'retained-skill-fixture'],
    ['/home/cma/.cma/provider/codex/auth.json', '{"fixture":"retained-auth-fixture"}'],
    [`${agentHome}/tmp/retained`, 'retained-other-temporary-content'],
    [`${agentHome}/tmp/arg0-sibling/retained`, 'retained-adjacent-content'],
  ];
  for (const [path, bytes] of retained) {
    mkdirSync(join(path, '..'), { recursive: true });
    writeFileSync(path, bytes, { mode: 0o600 });
  }
  symlinkSync('/home/cma/.cma/provider/codex/auth.json', `${agentHome}/auth.json`);
  assert.equal(execFileSync('codex', ['--version'], {
    encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, HOME: agentHome, CODEX_HOME: agentHome },
  }).trim(), artifacts.codexVersion);
  const aliasRoot = `${agentHome}/tmp/arg0`;
  const aliases = readdirSync(aliasRoot, { recursive: true, withFileTypes: true }).filter((entry) => entry.isSymbolicLink());
  assert.deepEqual(aliases.map((entry) => entry.name).sort(), ['apply_patch', 'applypatch', 'codex-execve-wrapper', 'codex-linux-sandbox']);
  for (const entry of aliases) assert.equal(readlinkSync(join(entry.parentPath, entry.name)), '/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex');
  complete('agent-home-helper-aliases');
  const toolDirectory = '/home/cma/ci-user-tools/bin with spaces';
  mkdirSync(toolDirectory, { recursive: true });
  writeFileSync(join(toolDirectory, 'cma-shell-path-fixture'), '#!/bin/sh\nprintf "cma-shell-path-ok\\n"\n', { mode: 0o700 });
  assertAgentShellPath(agentHome, toolDirectory);
  writeFileSync(join(agentHome, '.profile'), 'PATH="/home/cma/owner-profile-bin:$PATH"\nexport PATH\n');
  assert.equal(execFileSync('/bin/bash', ['-lc', '/usr/bin/printf "%s" "$PATH"'], {
    encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, HOME: agentHome, CODEX_HOME: agentHome },
  }), `/home/cma/owner-profile-bin:${process.env.PATH}`);
  complete('agent-shell-inherited-path');
  const rejectedHold = { t: 'checkpoint_for_teardown', checkpoint: { epoch: 1, exclusions: [] }, writer_resume_token: '0'.repeat(64) };
  assert.equal((await control(rejectedHold)).code, 'checkpoint_symlink_escape');
  await waitForWriterProgress(counter);
  complete('escaping-helper-refused');
  const hold = { t: 'checkpoint_for_teardown', checkpoint: { epoch: 1, exclusions: ['.cma/agents/ci-agent/home/tmp/arg0'] }, writer_resume_token: 'a'.repeat(64) };
  onStage('checkpoint-request');
  const receipt = await control(hold);
  assertCheckpointHoldReceipt(receipt, hold, onStage);
  onStage('checkpoint-writer-stable');
  const heldCounter = counter();
  await delay(150);
  assert.equal(counter(), heldCounter);
  complete('checkpoint-writers-held');
  assert.deepEqual(await control(hold), receipt);
  complete('checkpoint-exact-replay');
  assert.equal((await control({ t: 'resume_checkpoint_writers', writer_resume_token: 'b'.repeat(64) })).code, 'checkpoint_hold_conflict');
  complete('wrong-resume-token-refused');
  assert.equal((await control(credential)).code, 'checkpoint_writers_held');
  complete('mutation-while-held-refused');
  assert.match(receipt.checkpoint.sha256, /^[0-9a-f]{64}$/);
  assert.equal(receipt.checkpoint.object_reference, `home-${receipt.checkpoint.sha256}.cma`);
  const download = await fetch(`http://127.0.0.1:9090/runtime/checkpoints/${receipt.checkpoint.object_reference}`, { signal: AbortSignal.timeout(30000), redirect: 'error' });
  assert.equal(download.status, 200);
  const retainedArchive = Buffer.from(await download.arrayBuffer());
  assert.equal(sha256(retainedArchive), receipt.checkpoint.sha256);
  complete('checkpoint-download-digest');
  const resume = { t: 'resume_checkpoint_writers', writer_resume_token: hold.writer_resume_token };
  assert.equal((await control(resume)).writers_resumed, true);
  assert.ok(BigInt(await waitForWriterProgress(counter)) > BigInt(heldCounter));
  complete('writer-resumed');
  const archiveUrl = `http://127.0.0.1:9090/runtime/checkpoints/${receipt.checkpoint.object_reference}`;
  const archivePath = join(tmpdir(), 'cma-checkpoints', receipt.checkpoint.object_reference);
  assert.equal(existsSync(archivePath), false);
  assert.equal((await fetch(archiveUrl, { signal: AbortSignal.timeout(30000) })).status, 404);
  complete('checkpoint-local-release');
  assert.equal((await control(hold)).code, 'checkpoint_hold_retired');
  complete('retired-hold-refused');
  // Production imports the durable remote recovery point before each restore.
  // The retired transfer itself must not stay on the outgoing runtime's disk.
  const imported = await fetch(archiveUrl, {
    method: 'PUT', headers: { 'content-type': 'application/octet-stream' },
    body: retainedArchive, signal: AbortSignal.timeout(30000), redirect: 'error',
  });
  assert.equal(imported.status, 204);
  assert.equal(existsSync(archivePath), true);
  complete('checkpoint-reimport');
  for (const [path] of retained) writeFileSync(path, 'changed-after-checkpoint');
  assert.deepEqual(await control({ t: 'restore', object_reference: receipt.checkpoint.object_reference, sha256: receipt.checkpoint.sha256 }), { t: 'restore', accepted: true });
  for (const [path, bytes] of retained) assert.equal(readFileSync(path, 'utf8'), bytes);
  assert.equal(readlinkSync(`${agentHome}/auth.json`), '/home/cma/.cma/provider/codex/auth.json');
  assert.equal(readFileSync(`${agentHome}/auth.json`, 'utf8'), '{"fixture":"retained-auth-fixture"}');
  assert.equal(existsSync(aliasRoot), false);
  complete('whole-home-restore');
  assert.equal(existsSync(archivePath), false);
  assert.equal((await fetch(archiveUrl, { signal: AbortSignal.timeout(30000) })).status, 404);
  assert.equal(readdirSync(join(tmpdir(), 'cma-checkpoints')).some(name => /^\.restore-(?:staging|backup)-/.test(name)), false);
  complete('restore-local-release');
  assert.deepEqual(completed, checks);
  return { checks: completed, artifacts };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let stage = 'runtime-ready';
  try { process.stdout.write(`${JSON.stringify(await probe((value) => { stage = value; }))}\n`); }
  catch { process.stderr.write(`runtime image probe failed at ${stage}; raw runtime data withheld\n`); process.exitCode = 1; }
}
