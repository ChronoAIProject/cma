import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { call, mintBody } from './provider-test-support.mjs';

function launch(t, validToken = true) {
  const root = mkdtempSync(join(tmpdir(), 'ordinary-work-provider-cli-'));
  const bearer = randomBytes(32).toString('hex');
  const child = spawn(process.execPath, [fileURLToPath(new URL('provider-cli.mjs', import.meta.url)),
    '--state-root', root], { env: { PATH: process.env.PATH,
    ...(validToken ? { CMA_WORK_FIXTURE_BEARER: bearer } : {}) }, stdio: ['pipe', 'pipe', 'pipe'] });
  const exit = once(child, 'exit');
  const input = createInterface({ input: child.stdout });
  const lines = input[Symbol.asyncIterator]();
  let stderr = '';
  child.stderr.on('data', bytes => { stderr += bytes; });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await exit;
    input.close(); rmSync(root, { recursive: true });
  });
  return { bearer, root, child, exit, stderr: () => stderr,
    send: value => child.stdin.write(JSON.stringify(value) + '\n'),
    next: async () => {
      const result = await lines.next();
      assert.equal(result.done, false, 'fixture stdout ended early');
      assert(!result.value.includes(bearer));
      return JSON.parse(result.value);
    } };
}

test('executable fixture binds its sources and exposes a sanitized control channel', { timeout: 10000 }, async t => {
  const processFixture = launch(t);
  const ready = await processFixture.next();
  assert.equal(ready.type, 'ready');
  assert.match(ready.origin, /^http:\/\/127\.0\.0\.1:\d+$/);
  assert.equal(Object.keys(ready.sources).length, 4);
  for (const sha of Object.values(ready.sources)) assert.match(sha, /^[a-f0-9]{64}$/);
  processFixture.send({ command: 'configure', modes: { mint: 'hold_response' } });
  assert.equal((await processFixture.next()).ledger.modes.mint, 'hold_response');
  const response = call({ origin: ready.origin, bearer: processFixture.bearer },
    'POST', '/api/v1/api-keys', mintBody());
  let held;
  const deadline = performance.now() + 3000;
  while (!held && performance.now() < deadline) {
    processFixture.send({ command: 'snapshot' });
    held = (await processFixture.next()).ledger.pending.find(row => row.phase === 'response_held');
  }
  assert(held, 'accepted response barrier must be observed');
  processFixture.send({ command: 'release', request: held.request, action: 'drop' });
  const ledger = (await processFixture.next()).ledger;
  assert.equal(ledger.physical_commits, 1);
  assert.equal(ledger.received_mint_posts, 1);
  assert.equal(ledger.attempted_sends, null);
  assert.equal((await response.result).transportError, true);
  processFixture.send({ command: 'close' });
  assert.equal((await processFixture.next()).type, 'closed');
  assert.deepEqual(await processFixture.exit, [0, null]);
  assert.equal(processFixture.stderr(), '');
  assert(!existsSync(join(processFixture.root, 'provider.lock')));
});

test('invalid control fails closed, cleans its listener and never echoes payloads', { timeout: 10000 }, async t => {
  const processFixture = launch(t);
  await processFixture.next();
  processFixture.send({ command: 'proxy', credential: processFixture.bearer });
  assert.deepEqual(await processFixture.next(), { type: 'error', code: 'fixture_control_refused' });
  assert.equal((await processFixture.next()).reason, 'invalid_control');
  assert.deepEqual(await processFixture.exit, [1, null]);
  assert.equal(processFixture.stderr(), '');
  assert(!existsSync(join(processFixture.root, 'provider.lock')));
});

test('missing runtime-supplied token refuses startup without retaining a lock', { timeout: 10000 }, async t => {
  const processFixture = launch(t, false);
  assert.deepEqual(await processFixture.next(), { type: 'error', code: 'fixture_start_failed' });
  assert.equal((await processFixture.next()).reason, 'startup_failed');
  assert.deepEqual(await processFixture.exit, [1, null]);
  assert(!existsSync(join(processFixture.root, 'provider.lock')));
});
