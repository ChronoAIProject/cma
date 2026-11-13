import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { LocalHttp } from '../lib/http.mjs';
import { LocalCluster, executeWithInput } from '../lib/kubernetes.mjs';
import { hash } from '../lib/checks.mjs';
import { saveReport } from '../lib/report.mjs';

test('real loopback HTTP records full-body latency and retains errors without response contents', async () => {
  const server = createServer((request, response) => {
    if (request.url === '/redirect') { response.writeHead(302, { Location: 'https://remote.invalid' }); response.end(); }
    else if (request.url === '/bad') { response.writeHead(200); response.end('malformed'); }
    else { response.writeHead(200, { 'Content-Type': 'application/json' }); response.end('{"safe":true}'); }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const client = new LocalHttp(`http://127.0.0.1:${server.address().port}`);
  try {
    assert.equal((await client.request('/ok', { route: '/ok' })).value.safe, true);
    await assert.rejects(client.request('/redirect', { route: '/redirect' }));
    await assert.rejects(client.request('/bad', { route: '/bad' }));
    await assert.rejects(client.request('//remote.invalid', { route: '/bad' }));
    await assert.rejects(client.request('/ok', { method: 'POST', route: '/ok' }));
    await assert.rejects(client.request('/ok'));
    assert.equal((await client.request('/ok', { method: 'POST', key: 'synthetic-operation', body: {}, route: '/ok' })).status, 200);
    assert.equal((await client.command('synthetic-command')).status, 200);
    assert.throws(() => client.command('../bad'));
    assert.equal(client.samples.length, 5); assert.equal(client.samples.filter((s) => !s.ok).length, 2);
    assert.ok(!JSON.stringify(client.samples).includes('malformed'));
  } finally { await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); }
});

test('Kubernetes wrapper pins local API server, namespace UID and test-owned pod; refuses Secret and exec', async () => {
  const calls = [];
  const pod = { metadata: { name: 'test-am', uid: 'test-pod', labels: { 'app.kubernetes.io/part-of': 'cma-scale-tests' } } };
  let remote = false;
  const command = async (exe, args, options) => {
    calls.push({ executable: exe, args, input: options.input });
    const value = args.includes('view') ? remote ? 'https://remote.invalid' : 'https://127.0.0.1:6443'
      : args.includes('namespace') ? JSON.stringify({ metadata: { name: 'cma-scale', uid: 'test-namespace' } })
        : args.includes('get') ? JSON.stringify({ items: [pod] }) : '';
    return { stdout: value };
  };
  const config = { kubeconfig: '/test/private/config', context: 'kind-cma-scale', namespace: 'cma-scale', namespaceHash: hash('test-namespace') };
  const cluster = new LocalCluster(config, command);
  assert.equal((await cluster.read('pods')).items.length, 1);
  await assert.rejects(cluster.read('secrets')); await assert.rejects(cluster.read('exec'));
  const target = { name: 'test-am', uidHash: hash('test-pod'), labels: pod.metadata.labels };
  assert.equal((await cluster.removePod(target)).processDeathObserved, false);
  assert.deepEqual(calls.at(-1).args.slice(-5),
    ['delete', '--raw', '/api/v1/namespaces/cma-scale/pods/test-am', '-f', '-']);
  assert.deepEqual(JSON.parse(calls.at(-1).input), { apiVersion: 'v1', kind: 'DeleteOptions',
    gracePeriodSeconds: 0, preconditions: { uid: 'test-pod' } });
  await assert.rejects(cluster.removePod({ ...target, uidHash: hash('replaced') }));
  await assert.rejects(cluster.removePod({ ...target, labels: {} }));
  remote = true; await assert.rejects(cluster.read('pods'), /nonlocal/);
  assert.throws(() => new LocalCluster({ ...config, context: 'prod' }, command));
});

test('pod name reuse between GET and DELETE conflicts on the original UID', async () => {
  let currentUid = 'original';
  let deleted = false;
  const labels = { 'app.kubernetes.io/part-of': 'cma-scale-tests' };
  const command = async (exe, args, options) => {
    assert.equal(exe, 'kubectl');
    if (args.includes('view')) return { stdout: 'https://127.0.0.1:6443' };
    if (args.includes('namespace')) return { stdout: JSON.stringify({ metadata: { name: 'cma-scale', uid: 'ns' } }) };
    if (args.includes('get')) {
      const response = { items: [{ metadata: { name: 'test-am', uid: currentUid, labels } }] };
      currentUid = 'replacement';
      return { stdout: JSON.stringify(response) };
    }
    assert.ok(args.includes('--raw'));
    const body = JSON.parse(options.input);
    assert.equal(body.preconditions.uid, 'original');
    if (body.preconditions.uid !== currentUid) throw new Error('Conflict: UID precondition failed');
    deleted = true;
    return { stdout: '{}' };
  };
  const cluster = new LocalCluster({ kubeconfig: '/fixture/config', context: 'kind-cma-scale',
    namespace: 'cma-scale', namespaceHash: hash('ns') }, command);
  await assert.rejects(cluster.removePod({ name: 'test-am', uidHash: hash('original'), labels }), /Conflict/);
  assert.equal(deleted, false);
});

test('raw command transport delivers stdin and propagates command refusal', async () => {
  const options = { input: '{"preconditions":{"uid":"synthetic"}}', timeout: 2000, encoding: 'utf8' };
  const result = await executeWithInput(process.execPath, ['-e', 'process.stdin.pipe(process.stdout)'], options);
  assert.equal(result.stdout, options.input);
  await assert.rejects(executeWithInput(process.execPath, ['-e', 'process.exit(1)'], options));
});

test('reports retain explicit blocked rows and performance deltas', async () => {
  const dir = await mkdtemp(join(import.meta.dirname, '.report-'));
  try {
    await saveReport(dir, { status: 'BLOCKED', scenarios: [{ scenario: 'cross_replica', status: 'BLOCKED', code: 'fixture_missing' }],
      performance: [{ metric: '/route', phase: 'n2', n: 100, failed: 0, p50_ms: 1, p95_ms: 2,
        p99_ms: null, delta_ms: 1, delta_percent: 100, noise_ms: 0, status: 'FAIL' }] });
    const text = await readFile(join(dir, 'RESULTS.md'), 'utf8');
    assert.match(text, /fixture_missing/); assert.match(text, /unmeasured/); assert.match(text, /SHA-256/);
  } finally { await rm(dir, { recursive: true }); }
});
