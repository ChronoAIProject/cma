import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createServer } from 'node:http';
import test from 'node:test';
import { catalogSha256 } from '../lib/catalog.mjs';
import { hash } from '../lib/checks.mjs';
import { verifyLocalCohort } from '../lib/local-cohort.mjs';

const sourceSha = 'a'.repeat(40);
const incarnation = 'b'.repeat(64);
const cursor = `ag1.${incarnation}.0`;

function configFor(origin, count) {
  const owners = Array.from({ length: count }, (_, index) => ({
    agent: `agt_example_${index}`, sandbox: `sbx_example_${index}`, subjectHash: hash(`owner-${index}`),
  }));
  return {
    schema: 'cma-scale-v1', environment: 'local', isolated: true,
    context: 'kind-cma-scale', namespace: 'cma-scale', endpoints: { am: origin },
    hardwareHash: hash('hardware'), fixtureHash: hash(JSON.stringify(owners)), workloadHash: hash('workload'),
    sourceHash: hash(sourceSha), imagesHash: hash('images'), catalogSha256,
    recoveryMs: 30_000, scaleTimeoutMs: 60_000,
    localCohort: { sourceSha, owners, cluster: { kubeconfig: '/synthetic/kubeconfig', namespaceHash: hash('namespace') } },
  };
}

async function cluster(_file, args) {
  if (args.includes('view')) return { stdout: 'https://127.0.0.1:26443' };
  assert.ok(args.includes('namespace'));
  return { stdout: JSON.stringify({ metadata: { name: 'cma-scale', uid: 'namespace' } }) };
}

async function fixture(t, count, { wrongSubject, wrongStream, closeEarly, changeBuild } = {}) {
  const env = Object.fromEntries(Array.from({ length: count }, (_, index) =>
    [`CMA_LOCAL_SCALE_BEARER_${index + 1}`, `synthetic-local-owner-${index}`]));
  const streams = new Set();
  const seen = { builds: 0, peak: 0, openAtFinalBuild: null, write: 0 };
  const server = createServer((request, response) => {
    if (request.method !== 'GET') seen.write += 1;
    const match = /^Bearer synthetic-local-owner-(\d+)$/.exec(request.headers.authorization);
    const index = match ? Number(match[1]) : -1;
    assert.ok(index >= 0 && index < count);
    const path = request.url;
    const agent = `agt_example_${index}`;
    const sandbox = `sbx_example_${index}`;
    const events = `/api/v2/agents/${agent}/events`;
    const json = (value) => response.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(value));
    if (path === '/api/v1/build-info') {
      seen.builds += 1;
      if (seen.builds > 1) seen.openAtFinalBuild = streams.size;
      return json({ source_sha: changeBuild && seen.builds > 1 ? 'c'.repeat(40) : sourceSha });
    }
    if (path === '/auth/session') return json({
      authenticated: true, visitor: false, subject: `owner-${wrongSubject && index === 1 ? 0 : index}`,
    });
    if (path === `/api/v1/agents/${agent}`) return json({ id: agent, status: 'ready', sandbox: { id: sandbox } });
    if (path === `/api/v1/sandboxes/${sandbox}`) return json({ id: sandbox, status: 'active', owner: `owner-${index}` });
    if (path === `${events}/head`) return json({
      agent_id: agent, incarnation, through_cursor: cursor, active_turn: null,
    });
    if (path === `${events}?after_cursor=${cursor}&follow=true`) {
      streams.add(response);
      seen.peak = Math.max(seen.peak, streams.size);
      response.on('close', () => streams.delete(response));
      const frame = { agent_id: wrongStream && index === 1 ? 'agt_other' : agent, incarnation,
        reason: 'reconnected', frames: null, turn_sequence: null };
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.write(`event: cma.agent.live_gap\ndata: ${JSON.stringify(frame)}\n\n`);
      if (closeEarly && index === 0) response.end();
      return;
    }
    response.writeHead(404).end();
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => {
    server.close();
    server.closeAllConnections();
  });
  const config = configFor(`http://127.0.0.1:${server.address().port}`, count);
  return { config, env, seen };
}

test('observes all 100 independently authenticated streams together on the expected build', async (t) => {
  const { config, env, seen } = await fixture(t, 100);
  assert.deepEqual(await verifyLocalCohort(config, { env, command: cluster }), {
    verifiedOwners: 100, simultaneousStreams: 100, sourceSha, fixtureHash: config.fixtureHash,
  });
  assert.deepEqual(seen, { builds: 2, peak: 100, openAtFinalBuild: 100, write: 0 });
});

for (const [setting, code] of [
  ['wrongSubject', 'local_cohort_session_mismatch'],
  ['wrongStream', 'local_cohort_stream_identity_changed'],
  ['closeEarly', 'local_cohort_stream_closed'],
  ['changeBuild', 'local_cohort_build_changed'],
]) {
  test(`refuses cohort evidence when ${setting}`, async (t) => {
    const { config, env, seen } = await fixture(t, 2, { [setting]: true });
    await assert.rejects(verifyLocalCohort(config, { env, command: cluster }), { code });
    assert.equal(seen.write, 0);
  });
}

test('refuses nonloopback application and Kubernetes targets before application requests', async () => {
  let transportCalls = 0;
  const fetchImpl = () => { transportCalls += 1; };
  const env = { CMA_LOCAL_SCALE_BEARER_1: 'synthetic-local-owner-0', CMA_LOCAL_SCALE_BEARER_2: 'synthetic-local-owner-1' };
  await assert.rejects(verifyLocalCohort(configFor('https://example.com', 2), { env, command: cluster, fetchImpl }),
    { code: 'nonlocal_target_refused' });
  const command = async () => ({ stdout: 'https://example.com' });
  await assert.rejects(verifyLocalCohort(configFor('https://127.0.0.1', 2), { env, command, fetchImpl }),
    { code: 'nonlocal_target_refused' });
  assert.equal(transportCalls, 0);
});

test('refuses a changed fixture manifest and reused owner credentials before requests', async () => {
  const config = configFor('https://127.0.0.1', 2);
  const env = { CMA_LOCAL_SCALE_BEARER_1: 'synthetic-local-owner-0', CMA_LOCAL_SCALE_BEARER_2: 'synthetic-local-owner-0' };
  await assert.rejects(verifyLocalCohort(config, { env }), { code: 'local_cohort_owner_reused' });
  config.localCohort.owners[0].subjectHash = hash('another-owner');
  await assert.rejects(verifyLocalCohort(config, { env }), { code: 'local_cohort_binding_missing' });
});
