import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runBounded } from './install-contract-browser.mjs';

// A killed process reparented to an init that never reaps (a container's
// PID 1) stays a zombie; it still answers signal 0 but runs nothing.
function gone(pid) {
  try {
    process.kill(pid, 0);
  } catch (error) {
    return error.code === 'ESRCH';
  }
  try {
    return readFileSync(`/proc/${pid}/stat`, 'utf8').split(') ')[1]?.startsWith('Z') ?? false;
  } catch {
    return false;
  }
}

async function waitForStopped(
  isStopped,
  pause = (ms) => new Promise((done) => setTimeout(done, ms)),
) {
  for (let wait = 0; wait < 50; wait += 1) {
    if (isStopped()) return true;
    await pause(100);
  }
  return isStopped();
}

test('bounded downloader returns combined output and reports a failing exit', async () => {
  const script = "console.log('stdout row'); console.error('stderr row');";
  assert.match(
    await runBounded(process.execPath, ['-e', script], { timeoutMs: 10_000 }),
    /stdout row[\s\S]*stderr row|stderr row[\s\S]*stdout row/,
  );
  const failure = await runBounded(process.execPath, ['-e', `${script} process.exit(3)`], {
    timeoutMs: 10_000,
  }).catch((error) => error);
  assert.match(failure.message, /exit code 3/);
  assert.match(failure.output, /stdout row/);
  assert.match(failure.output, /stderr row/);
});

test('a stopped worker observation survives a later reaping race', async () => {
  const observations = [true, false];
  let checks = 0;
  const stopped = await waitForStopped(
    () => observations[checks++],
    () => assert.fail('a stopped worker needs no further wait'),
  );
  assert.equal(stopped, true);
  assert.equal(checks, 1);
});

test('worker cleanup waits for a stopped observation', async () => {
  let checks = 0;
  const waits = [];
  assert.equal(
    await waitForStopped(
      () => ++checks === 3,
      async (ms) => waits.push(ms),
    ),
    true,
  );
  assert.equal(checks, 3);
  assert.deepEqual(waits, [100, 100]);
});

test('a running worker exhausts the bounded cleanup wait without passing', async () => {
  let checks = 0;
  const waits = [];
  const stopped = await waitForStopped(
    () => {
      checks += 1;
      return false;
    },
    async (ms) => waits.push(ms),
  );
  assert.equal(stopped, false);
  assert.equal(checks, 51);
  assert.deepEqual(waits, Array(50).fill(100));
});

test(
  'a timed-out downloader is stopped together with the worker it forked',
  { timeout: 20_000 },
  async () => {
    const worker = 'setInterval(() => {}, 1000)';
    const script =
      `const child = require('node:child_process').spawn(process.execPath, ['-e', ${JSON.stringify(worker)}], { stdio: 'inherit' });` +
      "console.log('worker ' + child.pid); setInterval(() => {}, 1000);";
    const started = Date.now();
    const failure = await runBounded(process.execPath, ['-e', script], { timeoutMs: 2_000 }).catch(
      (error) => error,
    );
    assert.equal(failure.code, 'ETIMEDOUT');
    assert.ok(Date.now() - started < 10_000);
    const pid = Number(/worker (\d+)/.exec(failure.output)?.[1]);
    assert.ok(pid > 0, failure.output);
    assert.ok(await waitForStopped(() => gone(pid)), `forked worker ${pid} survived the timeout`);
  },
);
