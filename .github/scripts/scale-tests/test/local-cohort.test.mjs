import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { catalogSha256 } from '../lib/catalog.mjs';
import { hash } from '../lib/checks.mjs';
import { verifyLocalCohort } from '../lib/local-cohort.mjs';
import { runLocalWorkload } from '../lib/local-workload.mjs';
import { localHpaSelection, localLossSelection, runLocalResilience } from '../lib/local-resilience.mjs';

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

async function fixture(t, count, { wrongSubject, wrongStream, closeEarly, changeBuild, workloadFault } = {}) {
  const env = Object.fromEntries(Array.from({ length: count }, (_, index) =>
    [`CMA_LOCAL_SCALE_BEARER_${index + 1}`, `synthetic-local-owner-${index}`]));
  const streams = new Map();
  const commands = new Map();
  const offerKeys = new Set();
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
      streams.set(index, response);
      seen.peak = Math.max(seen.peak, streams.size);
      response.on('close', () => streams.delete(index));
      const frame = { agent_id: wrongStream && index === 1 ? 'agt_other' : agent, incarnation,
        reason: 'reconnected', frames: null, turn_sequence: null };
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.write(`event: cma.agent.live_gap\ndata: ${JSON.stringify(frame)}\n\n`);
      if (closeEarly && index === 0) response.end();
      return;
    }
    if (path === `/api/v2/agents/${agent}/responses` && request.method === 'POST') {
      const key = request.headers['idempotency-key'];
      assert.match(key, /^scale-[a-f0-9]{64}$/);
      assert.equal(offerKeys.has(key), false);
      offerKeys.add(key);
      assert.equal(streams.size, count);
      const command = `cmd_example_${index}`;
      const id = `resp_example_${index}`;
      const responseView = { id, conversation: { id: agent }, status: 'completed' };
      commands.set(index, { responseView, command });
      response.writeHead(200, { 'Content-Type': 'text/event-stream',
        'x-cma-command-id': command, 'x-cma-response-id': id });
      response.end(`data: ${JSON.stringify({
        type: 'response.completed', sequence_number: 1, response: responseView,
      })}\n\n`);
      const sequence = workloadFault === 'gap' ? 2 : 1;
      const idCursor = `ag1.${incarnation}.${sequence}`;
      streams.get(index).write(`event: cma.agent.event\nid: ${idCursor}\ndata: ${JSON.stringify({
        agent_id: agent, incarnation, sequence, cursor: idCursor, kind: { type: 'turn_completed', turn_sequence: 1 },
      })}\n\n`);
      return;
    }
    const admitted = commands.get(index);
    if (admitted && path === `/api/v2/commands/${admitted.command}`) {
      return json({ receipt: { command_id: admitted.command, agent_id: agent, response_id: admitted.responseView.id },
        status: 'completed', turn_sequence: 1 });
    }
    if (admitted && path === `/api/v2/responses/${admitted.responseView.id}`) {
      return json(workloadFault === 'durable' ? { ...admitted.responseView, status: 'failed' } : admitted.responseView);
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

function acceleratedClock() {
  let current = 0;
  return { now: () => current, sleep: async (ms, value, options) => {
    await sleep(1, value, options);
    current += ms;
  } };
}

test('runs the 100-owner workload at the measured rates with traceable command receipts', async (t) => {
  const { config, env, seen } = await fixture(t, 100);
  config.localWorkload = { durationMs: 60_000 };
  const report = await runLocalWorkload(config, { env, command: cluster, time: acceleratedClock() });
  assert.equal(report.status, 'PASS', report.code);
  assert.equal(report.openStreams, 100);
  assert.deepEqual(report.minuteRates, [{ readsPerSecond: 10, turnsPerMinute: 20 }]);
  assert.equal(seen.openAtFinalBuild, 100);
  assert.equal(seen.write, 20);
  assert.equal(report.samples.length, 620);
  const turns = report.attempts.filter(({ kind }) => kind === 'turn');
  assert.equal(new Set(turns.map(({ commandHash }) => commandHash)).size, 20);
  assert.ok(turns.every(({ commandHash, keyHash, status }) => commandHash.length === 64
    && keyHash.length === 64 && status === 'PASS'));
  assert.doesNotMatch(JSON.stringify(report), /synthetic-local-owner|agt_example|cmd_example|resp_example/);
});

for (const [workloadFault, code] of [['gap', 'local_cohort_cursor_gap'], ['durable', 'local_workload_request_failed']]) {
  test(`retains failed attempts when the live workload sees ${workloadFault}`, async (t) => {
    const { config, env, seen } = await fixture(t, 100, { workloadFault });
    config.localWorkload = { durationMs: 60_000 };
    const report = await runLocalWorkload(config, { env, command: cluster, time: acceleratedClock() });
    assert.equal(report.status, 'FAIL');
    assert.equal(report.code, code);
    assert.ok(seen.write > 0);
    assert.ok(report.attempts.length > 0);
    if (workloadFault === 'gap') assert.equal(seen.write, 1);
    else assert.ok(report.attempts.some(({ code }) => code === 'local_workload_durable_response_mismatch'));
  });
}

test('checks scope and the full 100-owner fixture before the first workload request', async () => {
  const config = configFor('https://127.0.0.1', 2);
  config.localWorkload = { durationMs: 60_000 };
  const fetchImpl = () => assert.fail('unexpected application request');
  assert.equal((await runLocalWorkload(config, { fetchImpl })).code, 'local_workload_100_owners_required');
  const remote = configFor('https://example.com', 100);
  remote.localWorkload = config.localWorkload;
  assert.equal((await runLocalWorkload(remote, { fetchImpl })).code, 'nonlocal_target_refused');
});

test('the scale CLI records the blocked workload and protects the evidence directory', async (t) => {
  const temp = await mkdtemp(join(tmpdir(), 'scale-workload-'));
  t.after(() => rm(temp, { recursive: true, force: true }));
  const path = join(temp, 'config.json');
  const output = join(temp, 'evidence');
  const config = configFor('https://example.com', 100);
  config.localWorkload = { durationMs: 60_000 };
  await writeFile(path, JSON.stringify(config), { mode: 0o600 });
  const runner = fileURLToPath(new URL('../run.mjs', import.meta.url));
  const launch = () => promisify(execFile)(process.execPath,
    [runner, '--scenario', 'connected-workload', '--config', path, '--out', output]);
  await assert.rejects(launch(), { code: 2 });
  const file = join(output, 'results.json');
  const retained = await readFile(file, 'utf8');
  const report = JSON.parse(retained);
  assert.equal(report.status, 'NO_GO');
  assert.deepEqual(report.scenarios.map(({ scenario, status, code }) => ({ scenario, status, code })), [
    { scenario: 'connected_workload', status: 'BLOCKED', code: 'nonlocal_target_refused' },
  ]);
  await assert.rejects(launch(), { code: 2 });
  assert.equal(await readFile(file, 'utf8'), retained);
});


async function boundHooks(t, config, implementation, { captureCount = config.localWorkload ? 20 : 1,
  authorityProcess = hash('process'), terminalSequence = 5, durableObservedSequence = terminalSequence ?? 6 } = {}) {
  const directory = await realpath(await mkdtemp(join(tmpdir(), 'local-resilience-')));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const path = join(directory, 'hooks.mjs');
  const contents = 'export const createLocalHooks = null;';
  await writeFile(path, contents, { mode: 0o600 });
  config.localResilience = { hookModule: path, hookSha256: hash(contents) };
  const custody = [];
  let captureRun;
  const hooks = {
    async preflight({ namespaceHash }) {
      return { ready: true, namespaceHash, sourceHash: config.sourceHash, evidenceHash: hash('fixture') };
    },
    async beginCapture({ runHash }) { captureRun = runHash; custody.push('begin'); },
    async finishCapture({ runHash }, options) {
      assert.equal(Object.hasOwn(options, 'acceptedHashes'), false);
      assert.equal(runHash, captureRun);
      custody.push('finish');
      const captures = {};
      const rows = {
        admissions: { accepted: true, observedSequence: 2 },
        authority: { fromSequence: 1, throughSequence: 6, processHash: authorityProcess },
        durable: { observedSequence: durableObservedSequence, terminal: true, terminalSequence },
        receiver: { observedSequence: 4, terminal: true },
      };
      for (const [role, fields] of Object.entries(rows)) {
        const ids = Array.from({ length: captureCount }, (_, n) => `cmd_example_${n}`);
        const records = ids.map((commandId) => ({
          commandHash: hash(commandId), commandId, ownerHash: hash('owner'),
          runtimeGeneration: 1, authorityEpoch: 1, ...fields,
        }));
        const path = join(directory, role + '.json');
        const bytes = JSON.stringify({ schema: 'cma-local-outcome-capture-v1', role, runHash,
          sourceHash: hash(role), complete: true, records });
        await writeFile(path, bytes, { mode: 0o600 });
        captures[role] = { path, sha256: hash(bytes) };
      }
      return captures;
    },
    async closeCapture() { custody.push('close-capture'); },
    async close() { custody.push('close-hooks'); },
    ...implementation,
  };
  return { custody, importer: async () => ({ createLocalHooks: async () => hooks }) };
}

function clusterResources(config, phase, { wrongRole = false } = {}) {
  const resource = (role, kind) => `${role}-${kind}`;
  config.localResilience.hpa = Object.fromEntries(['am', 'eg'].map((role) =>
    [role, Object.fromEntries(['deployment', 'hpa', 'pdb'].map((kind) => [kind, hash(resource(role, kind))]))]));
  return async (file, args) => {
    const stage = phase();
    const count = stage === 'up' ? 4 : 2;
    const scaleTime = stage === 'up' ? '2026-01-01T00:00:01Z'
      : stage === 'down' ? '2026-01-01T00:00:02Z' : null;
    const build = (kind, content) => ({ stdout: JSON.stringify({ items: ['am', 'eg'].map((role) => ({
      metadata: { uid: resource(role, kind), name: resource(role, kind), generation: 1 },
      ...content(role),
    })) }) });
    if (args.includes('deployments')) return build('deployment', (role) => ({
      spec: { replicas: count, template: { metadata: { labels: {
        role, 'app.kubernetes.io/name': role === 'eg' && !wrongRole ? 'cma-event-gateway' : 'cma-agent-manager',
      } } } },
      status: { observedGeneration: 1, replicas: count, updatedReplicas: count, readyReplicas: count },
    }));
    if (args.includes('horizontalpodautoscalers')) return build('hpa', (role) => ({
      spec: { minReplicas: 2, maxReplicas: 5, scaleTargetRef: { kind: 'Deployment', name: resource(role, 'deployment') } },
      status: { desiredReplicas: count, lastScaleTime: scaleTime, currentMetrics: [{}],
        conditions: [{ type: 'ScalingActive', status: 'True' }] },
    }));
    if (args.includes('poddisruptionbudgets')) return build('pdb', (role) => ({
      spec: { selector: { matchLabels: { role } } },
      status: { observedGeneration: 1, expectedPods: count, desiredHealthy: 1 },
    }));
    return cluster(file, args);
  };
}

async function waitForOffers(seen, previous) {
  for (let tries = 0; tries < 100 && seen.write <= previous; tries++) await sleep(5);
  assert.ok(seen.write > previous);
}

test('HPA orchestration holds the cohort through both controller changes and reconciles independent captures', async (t) => {
  const { config, env, seen } = await fixture(t, 100);
  config.localWorkload = { durationMs: 60_000 };
  let phase = 'initial';
  const { custody, importer } = await boundHooks(t, config, {
    async startStress() { const before = seen.write; await waitForOffers(seen, before); phase = 'up'; return 'synthetic-stress'; },
    async stopStress(receipt) {
      assert.equal(receipt, 'synthetic-stress');
      const before = seen.write;
      await waitForOffers(seen, before);
      phase = 'down';
    },
  });
  const command = clusterResources(config, () => phase);
  const result = await runLocalResilience(config, localHpaSelection, { env, command, importer, time: acceleratedClock() });
  assert.equal(result.status, 'PASS', result.code);
  assert.equal(result.scale.up.am.ready, 4);
  assert.equal(result.scale.down.eg.ready, 2);
  assert.ok(result.scale.up.workload.scheduledTurns > result.scale.initial.workload.scheduledTurns);
  assert.ok(result.scale.down.workload.scheduledReads > result.scale.up.workload.scheduledReads);
  assert.equal(result.workload.reconciliation.commands, 20);
  assert.equal(seen.openAtFinalBuild, 100);
  assert.deepEqual(custody, ['begin', 'finish', 'close-capture', 'close-hooks']);
});

test('replica loss requires actual process death before recovery can be scored', async (t) => {
  const config = configFor('http://127.0.0.1:31991', 2);
  const ownerHash = hash('process');
  const { custody, importer } = await boundHooks(t, config, {
    async beginInFlight() { return { accepted: true, inFlight: true, leaseJustRenewed: true, ownerHash,
      commandHash: hash('cmd_example_0'), evidenceHash: hash('admission') }; },
    async injectLoss(work, { runHash }) {
      return { runHash, commandHash: work.commandHash, ownerHash, injectionSequence: 3, evidenceHash: hash('api') };
    },
    async observeDeath() { return { ownerHash, abrupt: true, processDeathObserved: false, evidenceHash: hash('runtime') }; },
    async observeRecovery() { assert.fail('recovery cannot be scored before independent process death'); },
    async confirmStream() {},
    async cleanupFault() { custody.push('close-fault'); },
  });
  config.localResilience.lossMode = 'am_command';
  const result = await runLocalResilience(config, localLossSelection, { command: cluster, importer });
  assert.equal(result.code, 'abrupt_process_death_unproven');
  assert.deepEqual(custody, ['begin', 'close-fault', 'close-capture', 'close-hooks']);
});


for (const scenario of [
  { title: 'binds stream recovery to the original command', mode: 'am_stream' },
  { title: 'qualifies an effective loss before terminal even when death is observed later', mode: 'am_command' },
  { title: 'rejects loss after the command is terminal', mode: 'am_command', terminalSequence: 3,
    faultSequence: 5, error: 'oracle_fault_command_already_terminal' },
  { title: 'uses the durable transition even when the terminal read arrives after death', mode: 'am_command',
    terminalSequence: 4, durableObservedSequence: 6, faultSequence: 5, error: 'oracle_fault_command_already_terminal' },
  { title: 'rejects an effective loss at the terminal boundary', mode: 'am_command', terminalSequence: 4,
    error: 'oracle_fault_command_already_terminal' },
  { title: 'requires an independently ordered terminal transition', mode: 'am_command', terminalSequence: null,
    error: 'oracle_fault_terminal_sequence_missing' },
  { title: 'refuses death of a process outside the command authority interval', mode: 'am_stream',
    authorityProcess: hash('other-process'), error: 'oracle_fault_authority_mismatch' },
]) {
  test(`replica loss ${scenario.title}`, async (t) => {
    const config = configFor('http://127.0.0.1:31991', 2);
    const ownerHash = hash('process');
    const commandId = 'cmd_example_0';
    const faultSequence = scenario.faultSequence ?? 4;
    const { custody, importer } = await boundHooks(t, config, {
      async beginInFlight() { return { accepted: true, inFlight: true, leaseJustRenewed: true, ownerHash,
        commandHash: hash(commandId), evidenceHash: hash('admission') }; },
      async injectLoss(work, { runHash }) {
        return { runHash, commandHash: work.commandHash, ownerHash, injectionSequence: 3, evidenceHash: hash('api') };
      },
      async observeDeath(work, { runHash }) { return { runHash, commandHash: work.commandHash, ownerHash,
        authorityOwnerHash: hash('owner'), runtimeGeneration: 1, authorityEpoch: 1, faultSequence,
        deathObservedSequence: 6, abrupt: true, processDeathObserved: true, evidenceHash: hash('runtime') }; },
      async observeRecovery() { return { accounted: true, clientRecovered: true, sustainedSuccess: true }; },
      async confirmStream() { return { liveEvent: true, durableContinuity: true }; },
      async cleanupFault() { custody.push('close-fault'); },
    }, scenario);
    config.localResilience.lossMode = scenario.mode;
    const fetchImpl = async (url, options) => {
      assert.equal(url, `${config.endpoints.am}/api/v2/commands/${commandId}`);
      assert.equal(options.method, 'GET');
      return new Response(JSON.stringify({ receipt: { command_id: commandId }, status: 'completed' }));
    };
    const result = await runLocalResilience(config, localLossSelection, { command: cluster, importer, fetchImpl });
    if (scenario.error) {
      assert.equal(result.code, scenario.error);
      assert.notEqual(result.status, 'PASS');
    } else {
      assert.equal(result.status, 'PASS', result.code);
      assert.equal(result.fault.commands, 1);
      assert.equal(result.fault.observations, 1);
    }
    const { faultWitness } = result.fault;
    assert.equal(faultWitness.commandHash, hash(commandId));
    assert.equal(faultWitness.injectionSequence, 3);
    assert.equal(faultWitness.faultSequence, faultSequence);
    assert.equal(faultWitness.deathObservedSequence, 6);
    assert.equal(faultWitness.injectionEvidenceHash, hash('api'));
    assert.equal(faultWitness.deathEvidenceHash, hash('runtime'));
    assert.deepEqual(custody, ['begin', 'finish', 'close-fault', 'close-capture', 'close-hooks']);
  });
}

test('the collector cannot narrow the 20-command client receipt set', async (t) => {
  const { config, env } = await fixture(t, 100);
  config.localWorkload = { durationMs: 60_000 };
  const { importer } = await boundHooks(t, config, {}, { captureCount: 19 });
  const result = await runLocalResilience(config, 'connected-workload', { command: cluster, importer, env, time: acceleratedClock() });
  assert.equal(result.code, 'oracle_workload_admissions_mismatch');
  assert.equal(result.attempts.filter(({ kind }) => kind === 'turn').length, 20);
});

test('HPA refuses reused resources before opening the workload', async (t) => {
  const config = configFor('http://127.0.0.1:31991', 100);
  const { importer } = await boundHooks(t, config, {});
  clusterResources(config, () => 'initial');
  config.localResilience.hpa.eg = { ...config.localResilience.hpa.am };
  const result = await runLocalResilience(config, localHpaSelection, { command: cluster, importer });
  assert.equal(result.code, 'local_hpa_resource_binding_reused');
});

test('HPA rejects a distinct deployment advertising the wrong service role', async (t) => {
  const { config, env } = await fixture(t, 100);
  config.localWorkload = { durationMs: 60_000 };
  const { importer } = await boundHooks(t, config, { startStress() {}, stopStress() {} });
  const command = clusterResources(config, () => 'initial', { wrongRole: true });
  const result = await runLocalResilience(config, localHpaSelection, { command, importer, env, time: acceleratedClock() });
  assert.equal(result.code, 'local_hpa_deployment_role_mismatch');
});

test('HPA refuses a controller change after dispatch ends while a request is draining', async (t) => {
  const { config, env, seen } = await fixture(t, 100);
  config.localWorkload = { durationMs: 60_000 };
  const time = acceleratedClock();
  let phase = 'initial';
  const hold = Promise.withResolvers();
  let holding = false;
  const { importer } = await boundHooks(t, config, {
    async startStress({ signal }) {
      while (time.now() < 60_000) await sleep(2, undefined, { signal });
      assert.equal(holding, true);
      phase = 'up';
      return 'synthetic-stress';
    },
    async stopStress() { hold.resolve(); },
  });
  const command = clusterResources(config, () => phase);
  const fetchImpl = async (url, options) => {
    if (new URL(url).pathname === '/api/v1/agents/agt_example_99' && time.now() >= 59_900) {
      holding = true;
      await hold.promise;
    }
    return fetch(url, options);
  };
  const result = await runLocalResilience(config, localHpaSelection, { env, command, importer, time, fetchImpl });
  assert.equal(result.code, 'local_hpa_workload_ended_early');
  assert.equal(seen.write, 20);
});
