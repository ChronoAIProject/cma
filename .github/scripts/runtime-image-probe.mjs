import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';

export const checks = [
  'installed-artifacts', 'non-root-read-only', 'credential-before-install-refused',
  'install-exact-replay', 'credential-after-install', 'daemon-writer-active',
  'checkpoint-writers-held', 'checkpoint-exact-replay', 'wrong-resume-token-refused',
  'mutation-while-held-refused', 'checkpoint-download-digest', 'writer-resumed',
  'retired-hold-refused', 'whole-home-restore',
];
export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

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

export async function probe() {
  const completed = [];
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
  const artifacts = {};
  for (const [name, path] of Object.entries({
    runtime: '/usr/local/bin/layer4-runtime',
    launcher: '/usr/local/lib/node_modules/@openai/codex/bin/codex.js',
    native: '/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex',
    acp: '/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js',
  })) artifacts[name] = sha256(readFileSync(path));
  artifacts.codexVersion = execFileSync('codex', ['--version'], { encoding: 'utf8', timeout: 10000 }).trim();
  artifacts.acpVersion = JSON.parse(readFileSync('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json')).version;
  completed.push('installed-artifacts');
  assert.equal(process.getuid(), 10001);
  assert.throws(() => writeFileSync('/usr/local/cma-ci-write-refusal', 'refused'));
  completed.push('non-root-read-only');
  const credential = { t: 'credential', operation_id: 'ci-credential', value: secret, custody_generation: 1 };
  assert.equal((await control(credential)).code, 'install_not_completed');
  completed.push('credential-before-install-refused');

  // The detached writer remains an adopted supervisor child after installation.
  const writer = [
    'import os,time', 'pid=os.fork()', 'if pid: os._exit(0)', 'os.setsid()',
    'pid=os.fork()', 'if pid: os._exit(0)', 'n=0',
    'while True:', ' n+=1',
    ' with open("/home/cma/ci-writer", "w") as f: f.write(str(n))',
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
  completed.push('install-exact-replay');
  assert.deepEqual(await control(credential), { t: 'credential', operation_id: credential.operation_id, installed: true });
  completed.push('credential-after-install');
  const counter = () => readFileSync('/home/cma/ci-writer', 'utf8');
  await delay(100);
  const before = counter();
  await delay(100);
  assert.notEqual(counter(), before);
  completed.push('daemon-writer-active');
  mkdirSync('/home/cma/.cma/agents/ci-agent/home/sessions', { recursive: true });
  const retained = '/home/cma/.cma/agents/ci-agent/home/sessions/ci-history';
  writeFileSync(retained, 'retained-conversation-fixture', { mode: 0o600 });
  const hold = { t: 'checkpoint_for_teardown', checkpoint: { epoch: 1, exclusions: [] }, writer_resume_token: 'a'.repeat(64) };
  const receipt = await control(hold);
  assert.equal(receipt.t, hold.t);
  assert.equal(receipt.checkpoint.writers, 'held');
  assert.equal(receipt.writer_resume_token, hold.writer_resume_token);
  const heldCounter = counter();
  await delay(150);
  assert.equal(counter(), heldCounter);
  completed.push('checkpoint-writers-held');
  assert.deepEqual(await control(hold), receipt);
  completed.push('checkpoint-exact-replay');
  assert.equal((await control({ t: 'resume_checkpoint_writers', writer_resume_token: 'b'.repeat(64) })).code, 'checkpoint_hold_conflict');
  completed.push('wrong-resume-token-refused');
  assert.equal((await control(credential)).code, 'checkpoint_writers_held');
  completed.push('mutation-while-held-refused');
  assert.match(receipt.checkpoint.sha256, /^[0-9a-f]{64}$/);
  assert.equal(receipt.checkpoint.object_reference, `home-${receipt.checkpoint.sha256}.cma`);
  const download = await fetch(`http://127.0.0.1:9090/runtime/checkpoints/${receipt.checkpoint.object_reference}`, { signal: AbortSignal.timeout(30000), redirect: 'error' });
  assert.equal(download.status, 200);
  assert.equal(sha256(Buffer.from(await download.arrayBuffer())), receipt.checkpoint.sha256);
  completed.push('checkpoint-download-digest');
  const resume = { t: 'resume_checkpoint_writers', writer_resume_token: hold.writer_resume_token };
  assert.equal((await control(resume)).writers_resumed, true);
  await delay(150);
  assert.notEqual(counter(), heldCounter);
  completed.push('writer-resumed');
  assert.equal((await control(hold)).code, 'checkpoint_hold_retired');
  completed.push('retired-hold-refused');
  writeFileSync(retained, 'changed-after-checkpoint');
  assert.deepEqual(await control({ t: 'restore', object_reference: receipt.checkpoint.object_reference, sha256: receipt.checkpoint.sha256 }), { t: 'restore', accepted: true });
  assert.equal(readFileSync(retained, 'utf8'), 'retained-conversation-fixture');
  completed.push('whole-home-restore');
  assert.deepEqual(completed, checks);
  return { checks: completed, artifacts };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { process.stdout.write(`${JSON.stringify(await probe())}\n`); }
  catch { process.stderr.write('runtime image probe failed; raw runtime data withheld\n'); process.exitCode = 1; }
}
