const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const { mkdtemp, rm } = require('node:fs/promises');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { test } = require('node:test');

test('a production policy that denies browser namespaces reports unsupported without frames or a crashed mediator', {
  timeout: 30000, skip: process.env.CMA_BROWSER_POLICY_PROOF !== '1',
}, async () => {
  assert.equal(process.platform, 'linux');
  const directory = await mkdtemp(join(tmpdir(), 'cma-browser-policy-'));
  const mediator = spawn(process.execPath, [join(__dirname, 'mediator.cjs'), directory], {
    env: { PATH: process.env.PATH, HOME: directory }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  let diagnostic = '';
  mediator.stderr.on('data', bytes => { diagnostic += bytes; });
  const output = createInterface({ input: mediator.stdout });
  let sequence = 0;
  const call = async operation => {
    const identifier = ++sequence;
    const reply = once(output, 'line', { signal: AbortSignal.timeout(15000) });
    mediator.stdin.write(JSON.stringify({ id: identifier, operation, observer: 'policy-proof' }) + '\n');
    const value = JSON.parse((await reply)[0]);
    assert.equal(value.id, identifier);
    assert.equal(value.error, undefined, diagnostic);
    return value.value;
  };
  try {
    assert.equal((await call('status')).state, 'not_enabled');
    for (const operation of ['enable', 'status', 'observe']) {
      const observed = await call(operation);
      assert.equal(observed.state, 'unsupported', diagnostic);
      assert.equal(observed.context_id, null);
      assert.equal(observed.frame, null);
      assert.equal(observed.captures, 0);
      assert.equal(observed.observers, 0);
    }
    assert.equal((await call('disable')).state, 'not_enabled');
  } finally {
    const closed = once(mediator, 'exit', { signal: AbortSignal.timeout(5000) });
    mediator.stdin.end();
    try { await closed; } finally { mediator.kill('SIGKILL'); }
    output.close();
    await rm(directory, { recursive: true, force: true });
  }
});
