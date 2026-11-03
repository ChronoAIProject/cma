const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const { mkdtemp, rm } = require('node:fs/promises');
const { createServer, request: httpRequest } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');

async function fixture(test) {
  const directory = await mkdtemp(join(tmpdir(), 'cma-control-policy-'));
  const child = spawn(process.execPath, [join(__dirname, 'mediator.cjs'), directory], { stdio: ['pipe', 'pipe', 'pipe'] });
  child.stderr.resume();
  const pending = new Map();
  let serial = 0;
  createInterface({ input: child.stdout }).on('line', line => {
    const frame = JSON.parse(line);
    pending.get(frame.id)?.(frame);
  });
  const rpc = async parameters => {
    const id = ++serial;
    const result = new Promise(resolve => pending.set(id, resolve));
    let timeout;
    child.stdin.write(JSON.stringify({ id, ...parameters }) + '\n');
    try {
      return await Promise.race([result, new Promise((resolve, reject) => {
        timeout = setTimeout(() => reject(new Error('Browser RPC timeout')), 20000);
      })]);
    } finally { clearTimeout(timeout); pending.delete(id); }
  };
  const call = async parameters => {
    const frame = await rpc(parameters);
    assert.equal(frame.error, undefined, JSON.stringify(frame));
    return frame.value;
  };
  const values = [];
  const page = createServer(async (request, response) => {
    if (request.url === '/value') {
      let value = '';
      for await (const chunk of request) value += chunk;
      values.push(value);
      response.end('ok');
    } else {
      response.setHeader('content-type', 'text/html');
      response.end('<input autofocus style="position:absolute;left:20px;top:20px;width:500px;height:40px" oninput="fetch(\'/value\',{method:\'POST\',body:this.value})">');
    }
  });
  await new Promise(resolve => page.listen(0, '127.0.0.1', resolve));
  test.after(async () => {
    child.stdin.end();
    await new Promise(resolve => child.exitCode !== null ? resolve() : child.once('exit', resolve));
    page.closeAllConnections();
    await new Promise(resolve => page.close(resolve));
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  });
  const agent = (path, body) => new Promise((resolve, reject) => {
    const request = httpRequest({ socketPath: join(directory, 'browser.sock'), path, method: 'POST' }, response => {
      let text = '';
      response.on('data', bytes => { text += bytes; });
      response.on('end', () => resolve({ status: response.statusCode, ...JSON.parse(text) }));
    });
    request.on('error', reject);
    request.end(JSON.stringify(body));
  });
  const enabled = await call({ operation: 'enable' });
  assert.equal(enabled.state, 'loading');
  assert.equal((await agent('/navigate', { url: `http://127.0.0.1:${page.address().port}/form` })).status, 200);
  const base = { operation: 'control', context_id: enabled.context_id, actor: 'owner-proof', client_id: 'first-client' };
  const command = (command, revision, extra = {}) => ({ ...base, command, expected_revision: revision, operation_id: randomUUID(), ...extra });
  return { call, rpc, agent, command, base, values };
}

test('lost acquisition/input/return receipts recover exactly; a competing client and old lease cannot write', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  const acquire = browser.command('acquire', 0);
  const lease = await browser.call(acquire);
  const retry = await browser.call(acquire);
  assert.equal(retry.lease_token, lease.lease_token);
  assert.equal(retry.revision, lease.revision);
  const competing = await browser.rpc(browser.command('acquire', lease.revision, { client_id: 'second-client' }));
  assert.equal(competing.error, 'browser_control_conflict');
  const transferred = await browser.rpc(browser.command('input', lease.revision, {
    client_id: 'second-client', lease_token: lease.lease_token, input: { kind: 'text', text: 'BAD' },
  }));
  assert.equal(transferred.error, 'browser_control_conflict');
  const click = await browser.call(browser.command('input', lease.revision, {
    lease_token: lease.lease_token, input: { kind: 'click', x: 100, y: 40 },
  }));
  const input = browser.command('input', click.revision, { lease_token: lease.lease_token, input: { kind: 'text', text: 'ONCE' } });
  const written = await browser.call(input);
  assert.equal((await browser.call(input)).revision, written.revision);
  await delay(100);
  assert.deepEqual(browser.values, ['ONCE']);
  const recovered = await browser.call({ ...browser.base, command: 'status', operation_id: input.operation_id });
  assert.equal(recovered.operation_id, input.operation_id);
  const altered = await browser.rpc({ ...input, input: { kind: 'text', text: 'CHANGED' } });
  assert.equal(altered.error, 'browser_control_conflict');
  const release = browser.command('return', written.revision, { lease_token: lease.lease_token });
  const returned = await browser.call(release);
  assert.equal((await browser.call(release)).state, 'agent');
  assert.equal((await browser.rpc(browser.command('input', returned.revision, {
    lease_token: lease.lease_token, input: { kind: 'text', text: 'STALE' },
  }))).error, 'browser_control_conflict');
  const audit = await browser.call({ ...browser.base, command: 'status' });
  assert.equal(audit.lease_token, null);
  assert.deepEqual(audit.audit.map(event => event.kind), ['acquired', 'input', 'input', 'returned']);
  assert(!JSON.stringify(audit.audit).includes('ONCE'));
  assert(!JSON.stringify(audit.audit).includes(lease.lease_token));
});

test('offline lease expires before queued input, renew requires exact ownership, and replaced context rejects every old identity', { timeout: 40000 }, async test => {
  const browser = await fixture(test);
  const acquired = await browser.call(browser.command('acquire', 0));
  assert.equal(acquired.lease_duration_ms, 15000);
  assert(Date.parse(acquired.expires_at) > Date.now());
  const renewed = await browser.call(browser.command('renew', acquired.revision, { lease_token: acquired.lease_token }));
  assert.equal(renewed.state, 'user');
  assert.equal((await browser.agent('/navigate', { url: 'http://localhost/forbidden' })).error, 'browser_control_conflict');
  await delay(15100);
  const stale = await browser.rpc(browser.command('input', renewed.revision, {
    lease_token: renewed.lease_token, input: { kind: 'text', text: 'OFFLINE' },
  }));
  assert.equal(stale.error, 'browser_control_conflict');
  const expired = await browser.call({ ...browser.base, command: 'status' });
  assert.equal(expired.state, 'agent');
  assert.equal(expired.audit.at(-1).kind, 'expired');
  assert.equal((await browser.agent('/input', { input: { kind: 'text', text: 'RESUMED' } })).status, 200);
  await delay(100);
  assert.deepEqual(browser.values, ['RESUMED']);
  await browser.call({ operation: 'disable' });
  const enabled = await browser.call({ operation: 'enable' });
  assert.notEqual(enabled.context_id, browser.base.context_id);
  assert.equal((await browser.rpc(browser.command('acquire', 0))).error, 'browser_identity');
});

test('an old acquisition receipt never reveals a newer lease and bounded receipt eviction cannot replay a mutation', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  const first = browser.command('acquire', 0);
  const acquired = await browser.call(first);
  const returned = await browser.call(browser.command('return', acquired.revision, { lease_token: acquired.lease_token }));
  let latest = await browser.call(browser.command('acquire', returned.revision));
  assert.notEqual(latest.lease_token, acquired.lease_token);
  assert.equal((await browser.call(first)).lease_token, null);
  for (let index = 0; index < 130; index += 1) {
    latest = await browser.call(browser.command('renew', latest.revision, { lease_token: latest.lease_token }));
  }
  assert.equal(latest.audit.length, 128);
  assert(latest.audit_truncated_before > 1);
  assert.equal((await browser.rpc(first)).error, 'browser_control_conflict');
  assert.equal((await browser.rpc({ ...browser.base, command: 'status', operation_id: first.operation_id })).error, 'browser_operation_unknown');
});

test('bounded keyboard events edit the focused real form while unsupported shortcuts and script payloads are refused', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  let lease = await browser.call(browser.command('acquire', 0));
  for (const input of [{ kind: 'click', x: 100, y: 40 }, { kind: 'text', text: 'KEYS' }, { kind: 'key', key: 'Backspace' }]) {
    lease = await browser.call(browser.command('input', lease.revision, { lease_token: lease.lease_token, input }));
  }
  await delay(100);
  assert.equal(browser.values.at(-1), 'KEY');
  for (const input of [{ kind: 'key', key: 'F12' }, { kind: 'script', expression: 'document.body.remove()' }]) {
    assert.equal((await browser.rpc(browser.command('input', lease.revision, { lease_token: lease.lease_token, input }))).error, 'browser_input_invalid');
  }
  assert.equal((await browser.call({ ...browser.base, command: 'status' })).revision, lease.revision);
});
