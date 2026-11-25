const assert = require('node:assert/strict');
const { execFile, spawn } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const { once } = require('node:events');
const { mkdtemp, rm } = require('node:fs/promises');
const { createServer, request: httpRequest } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { test } = require('node:test');
const { setTimeout: delay } = require('node:timers/promises');
const { promisify } = require('node:util');

const run = promisify(execFile);

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
    } else if (request.url === '/heading-redirect') {
      response.writeHead(302, { location: '/heading-final' }).end('<h1>Redirect response</h1>');
    } else if (request.url === '/heading-final') {
      response.setHeader('content-type', 'text/html');
      response.end('<h1 id="heading">Example Domain</h1><h1>Second heading</h1>' +
        '<script>document.querySelector = () => ({ textContent: "Page override" });</script>');
    } else if (request.url === '/iframe-heading') {
      response.setHeader('content-type', 'text/html');
      response.end('<iframe src="/heading-final"></iframe>');
    } else if (request.url === '/empty-heading') {
      response.setHeader('content-type', 'text/html');
      response.end('<h1> </h1>');
    } else if (request.url === '/long-heading') {
      response.setHeader('content-type', 'text/html');
      response.end('<h1>' + 'H'.repeat(2000) + '</h1>');
    } else if (request.url === '/superseded-heading') {
      response.setHeader('content-type', 'text/html');
      response.end('<h1>Superseded document</h1><img src="/hold-load">' +
        '<script>setTimeout(() => location.replace("/form"), 200);</script>');
    } else if (request.url === '/hold-load') {
      response.writeHead(200, { 'content-type': 'image/png' }).flushHeaders();
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
  return { call, rpc, agent, command, base, values, directory, url: `http://127.0.0.1:${page.address().port}/form` };
}

test('Agent navigation reports the first H1 of the committed main document', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  const navigate = async path => {
    const { stdout } = await run(process.execPath, [
      join(__dirname, 'command.cjs'), 'navigate', new URL(path, browser.url).href,
    ], { env: { ...process.env, CMA_BROWSER_SOCKET: join(browser.directory, 'browser.sock') } });
    return JSON.parse(stdout);
  };

  for (const [path, heading] of [
    ['/heading-redirect', 'Example Domain'],
    ['/heading-final#heading', 'Example Domain'],
    ['/iframe-heading', null],
    ['/empty-heading', ''],
    ['/long-heading', 'H'.repeat(512)],
    ['/form', null],
  ]) {
    assert.deepEqual(await navigate(path), {
      navigated: true, location: 'Local page', first_heading: heading,
    }, path);
  }
  await assert.rejects(navigate('/superseded-heading'), {
    code: 1, stdout: /^\{"error":"browser_navigation_(unconfirmed|failed)"\}$/,
  });
  const status = await browser.call({ operation: 'status' });
  assert.equal(Object.hasOwn(status, 'first_heading'), false);
  const lease = await browser.call(browser.command('acquire', 0));
  assert.equal(lease.state, 'user');
  await assert.rejects(navigate('/heading-final'), {
    code: 1, stdout: '{"error":"browser_control_conflict"}',
  });
});

test('invalid navigation gives the Agent a recovery path and an unavailable browser reports disconnect', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  const navigate = url => run(process.execPath, [join(__dirname, 'command.cjs'), 'navigate', url], {
    env: { ...process.env, CMA_BROWSER_SOCKET: join(browser.directory, 'browser.sock') },
  });
  const overEnvelopeLimit = 'http://127.0.0.1/' + 'a'.repeat(16384);
  const refused = await browser.agent('/navigate', { url: overEnvelopeLimit });
  assert.equal(refused.status, 400);
  assert.equal(refused.error, 'browser_navigation_invalid');

  const partialBody = Buffer.from('{"url":"' + overEnvelopeLimit);
  let pendingRequest;
  try {
    const streamed = await new Promise((resolve, reject) => {
      pendingRequest = httpRequest({
        socketPath: join(browser.directory, 'browser.sock'),
        path: '/navigate',
        method: 'POST',
        headers: { 'content-length': partialBody.length + 2 },
        signal: AbortSignal.timeout(5000),
      }, response => {
        let body = '';
        response.setEncoding('utf8');
        response.on('data', chunk => { body += chunk; });
        response.on('error', reject);
        response.on('end', () => resolve({
          status: response.statusCode, connection: response.headers.connection, ...JSON.parse(body),
        }));
      });
      pendingRequest.on('error', reject);
      // Leave the closing quote and brace outstanding until the refusal arrives.
      pendingRequest.write(partialBody);
    });
    assert.equal(streamed.status, 400);
    assert.equal(streamed.connection, 'close');
    assert.equal(streamed.error, 'browser_navigation_invalid');
    assert.match(streamed.message, /HTTP or HTTPS URL without credentials/);
  } finally {
    pendingRequest?.destroy();
  }

  for (const url of ['file:///workspace/preview.html', 'not-a-url', 'https://user:placeholder@example.test/',
    'http://127.0.0.1/' + 'a'.repeat(8192), overEnvelopeLimit]) {
    await assert.rejects(navigate(url), error => {
      assert.equal(error.code, 1);
      const result = JSON.parse(error.stdout);
      assert.equal(result.error, 'browser_navigation_invalid');
      assert.match(result.message, /HTTP or HTTPS URL without credentials/);
      assert.match(result.message, /serve it over HTTP on 127\.0\.0\.1/);
      return true;
    });
  }
  assert.equal(JSON.parse((await navigate(browser.url)).stdout).navigated, true);
  await browser.call({ operation: 'disable' });
  await assert.rejects(navigate(browser.url), {
    code: 1, stdout: '{"error":"browser_disconnected"}',
  });
});

test('live observations report only the current origin and reset it with the browser context', { timeout: 30000 }, async test => {
  const browser = await fixture(test);
  const target = new URL(browser.url);
  target.pathname = '/private/document';
  target.search = '?search=private-value';
  target.hash = 'private-fragment';
  assert.equal((await browser.agent('/navigate', { url: target.href })).status, 200);
  const observed = await browser.call({ operation: 'observe', observer: 'origin-reader' });
  assert.equal(observed.location, 'Local page');
  assert.equal(observed.address, target.origin);
  assert.equal(observed.state, 'live');
  const disabled = await browser.call({ operation: 'disable' });
  assert.equal(disabled.address, 'about:blank');
  const enabled = await browser.call({ operation: 'enable' });
  assert.equal(enabled.address, 'about:blank');
  assert.notEqual(enabled.context_id, observed.context_id);
});

test('a native request with a delayed body cannot act in a replacement browser context', { timeout: 40000 }, async test => {
  const browser = await fixture(test);
  for (const path of ['/input', '/navigate']) {
    let complete;
    const result = new Promise(resolve => { complete = resolve; });
    const request = httpRequest({ socketPath: join(browser.directory, 'browser.sock'), path, method: 'POST',
      headers: { Expect: '100-continue' } }, response => {
      let body = '';
      response.on('data', bytes => { body += bytes; });
      response.on('end', () => complete({ status: response.statusCode, ...JSON.parse(body) }));
    });
    request.flushHeaders();
    await once(request, 'continue');
    request.write('{');
    const previous = await browser.call({ operation: 'status' });
    await browser.call({ operation: 'disable' });
    const replacement = await browser.call({ operation: 'enable' });
    assert.notEqual(previous.context_id, replacement.context_id);
    assert.equal((await browser.agent('/navigate', { url: browser.url })).status, 200);
    const value = path === '/input' ? { input: { kind: 'text', text: 'STALE-BODY' } } : { url: 'about:blank' };
    request.end(JSON.stringify(value).slice(1));
    assert.deepEqual(await result, { status: 409, error: 'browser_identity' });
  }
  await delay(100);
  assert.deepEqual(browser.values, []);
  assert.equal((await browser.agent('/input', { input: { kind: 'text', text: 'CURRENT' } })).status, 200);
  await delay(100);
  assert.deepEqual(browser.values, ['CURRENT']);
});

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
