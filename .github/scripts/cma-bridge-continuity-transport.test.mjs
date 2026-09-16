import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, symlink, watch as watchDirectory, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ObservationError } from './cma-bridge-continuity-errors.mjs';
import { executeReadOnly, transport } from './cma-bridge-continuity-transport.mjs';
import { config } from './cma-bridge-continuity-watch-fixture.mjs';

async function withToken(run) {
  const directory = await mkdtemp(join(tmpdir(), 'continuity-transport-'));
  const tokenFile = join(directory, 'token');
  await writeFile(tokenFile, 'fixture-private-token', { mode: 0o600 });
  try { return await run({ ...config, tokenFile }, directory); }
  finally { await rm(directory, { recursive: true, force: true }); }
}

test('transport only sends exact scoped GET requests and namespace-local kubectl reads', async () => {
  await withToken(async (settings) => {
    const requests = [];
    const commands = [];
    const reads = await transport(settings, {
      fetchImpl: async (url, options) => {
        requests.push({ url, options });
        return new Response(JSON.stringify({ items: [] }), { status: 200 });
      },
      execute: async (file, args, options) => { commands.push({ file, args, options }); return { stdout: '{"items":[]}' }; },
    });
    await reads.get('/', false);
    await reads.get('/api/v1/agents/agt_fixture', true);
    await reads.get('/api/v1/agents/agt_fixture/events?after=0&limit=100', true);
    const stop = new AbortController();
    await reads.pods({ signal: stop.signal });
    assert.ok(requests.every(({ url, options }) => url.origin === config.origin && options.method === 'GET' && options.redirect === 'error' && options.signal));
    assert.equal(requests[0].options.headers.Authorization, undefined);
    assert.ok(requests.slice(1).every(({ options }) => options.headers.Authorization === 'Bearer fixture-private-token'));
    assert.equal(commands.length, 1);
    assert.equal(commands[0].file, 'kubectl');
    assert.equal(commands[0].options.signal, stop.signal);
    assert.deepEqual(commands[0].args, ['--context', 'fixture', '--request-timeout=20s', '-n', 'disposable', 'get', 'pods', '-l', 'cma/control-plane=continuity', '-o', 'json']);
    for (const [path, authenticated] of [
      ['/api/v1/agents/agt_other', true], ['/api/v1/agents/agt_fixture/commands', true],
      ['/api/v1/agents/agt_fixture', false], ['/', true],
      ['/api/v1/agents/agt_fixture/events?after=0&limit=100&extra=1', true],
      ['/api/v1/agents/agt_fixture/events?after=0&limit=100?extra=1', true],
      ['https://other.example/api/v1/agents/agt_fixture', true],
    ]) await assert.rejects(reads.get(path, authenticated), /public_read_outside_scope/);
    assert.equal(requests.length, 3);
  });
});

test('cancellation reaches pending fetch and pending response-body consumption', async () => {
  await withToken(async (settings) => {
    for (const pending of ['fetch', 'body']) {
      const stop = new AbortController();
      let started;
      const ready = new Promise((resolve) => { started = resolve; });
      let aborts = 0;
      const reads = await transport(settings, {
        fetchImpl: async (url, { signal }) => {
          if (pending === 'fetch') return new Promise((resolve, reject) => {
            signal.addEventListener('abort', () => { aborts += 1; reject(new Error('private fetch error')); }, { once: true });
            started();
          });
          const body = new ReadableStream({
            start(controller) { signal.addEventListener('abort', () => { aborts += 1; controller.error(new Error('private body error')); }, { once: true }); },
            pull() { started(); },
          });
          return new Response(body, { status: 200 });
        },
      });
      const promise = reads.get('/api/v1/agents/agt_fixture', true, { signal: stop.signal });
      await ready;
      stop.abort(new ObservationError('watch_deadline'));
      await assert.rejects(promise, /watch_deadline/);
      assert.equal(aborts, 1);
    }
  });
});

test('pre-aborted reads never start HTTP or kubectl', async () => {
  await withToken(async (settings) => {
    const reads = await transport(settings, {
      fetchImpl: () => assert.fail('cancelled HTTP started'),
      execute: () => assert.fail('cancelled kubectl started'),
    });
    const stop = new AbortController();
    stop.abort();
    await assert.rejects(reads.get('/', false, { signal: stop.signal }), /observation_cancelled/);
    await assert.rejects(reads.pods({ signal: stop.signal }), /observation_cancelled/);
  });
});

test('subprocess cancellation kills and reaps a child even if it ignores SIGTERM', { timeout: 5000 }, async () => {
  await withToken(async (settings, directory) => {
    const stop = new AbortController();
    const marker = join(directory, 'ready.json');
    const changes = watchDirectory(directory, { signal: AbortSignal.timeout(4000) });
    const readiness = changes.next();
    const script = 'process.on("SIGTERM", () => {}); require("node:fs").writeFileSync(process.argv[1], JSON.stringify({pid:process.pid})); setInterval(() => {}, 1000);';
    const child = executeReadOnly(process.execPath, ['-e', script, marker], { signal: stop.signal, timeout: 4000, maxBuffer: 1024 });
    const rejection = assert.rejects(child, /observation_cancelled/);
    try {
      await readiness;
      let pid;
      for (;;) {
        try { pid = JSON.parse(await readFile(marker, 'utf8')).pid; break; }
        catch { await changes.next(); }
      }
      stop.abort();
      await rejection;
      assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
    } finally {
      stop.abort();
      await changes.return();
      await rejection;
    }
  });
});

test('response size and malformed public JSON fail without retaining response bodies', async () => {
  await withToken(async (settings) => {
    for (const [body, code] of [['private invalid json', 'invalid_public_json'], ['a'.repeat(4 * 1024 * 1024 + 1), 'public_response_too_large']]) {
      const reads = await transport(settings, { fetchImpl: async () => new Response(body, { status: 200 }) });
      await assert.rejects(reads.get('/api/v1/agents/agt_fixture', true), (error) => error.message === code);
    }
  });
});

test('saved credentials reject public files and symlinks', async () => {
  await withToken(async (settings, directory) => {
    const unsafe = join(directory, 'unsafe');
    await writeFile(unsafe, 'fixture-private-token', { mode: 0o644 });
    await assert.rejects(transport({ ...settings, tokenFile: unsafe }), /private_token_file_required/);
    const linked = join(directory, 'linked');
    await symlink(settings.tokenFile, linked);
    await assert.rejects(transport({ ...settings, tokenFile: linked }));
  });
});
