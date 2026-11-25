import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { CapacityClient, consumeEvents } from '../lib/lc01-client.mjs';
import { hash } from '../lib/checks.mjs';
import { end, loadPlan, protocols, steadyEnd, steadyStart, turnFixture, validateConfig, workloadShape } from '../lib/lc01-plan.mjs';
import { deliveryPlan, runCapacity } from '../lib/lc01-run.mjs';
import { distribution, scoreCapacity } from '../lib/lc01-score.mjs';

const origin = 'https://example.test';
const sha = hash('fixture\n');
const proof = hash('independent synthetic proof');
const sourceSha = 'a'.repeat(40);
const imageDigest = 'sha256:' + 'b'.repeat(64);
const streamHeaders = { 'Content-Type': 'text/event-stream; charset=utf-8' };
const cursor = (sequence) => `ag1.${proof}.${sequence}`;

function config() {
  return {
    schema: 'cma-lc01-staging-v1', origin, fixtureSha256: sha, candidate: { sourceSha, imageDigest },
    reservation: {
      isolated: true, verifiedK: 50, nPlusOneVerified: true, cohortHash: proof, backgroundPeakHash: proof,
      placementHash: proof, runtimeMappingHash: proof, providerBudgetHash: proof,
    },
    users: Array.from({ length: 50 }, (_, index) => ({
      subjectHash: hash(`owner-${index}`), agent: `agt_example${index}`, sandbox: `sbx_example${index}`,
      bearer: `synthetic-test-owner-${index}`, providerClass: [0, 4, 7, 20, 24, 27].includes(index) ? 'funded' : 'deterministic',
      modelClass: 'synthetic', runtimeEvidenceHash: proof, delaySeconds: 10 + index % 21,
    })),
    background: ['nyxid', 'direct-api'].map((consumer, index) => ({
      consumer, origin, agent: `agt_background${index}`, bearer: `synthetic-background-${index}`,
      prefix: index === 0 ? '/proxy' : '', peakReadsPerSecond: 2,
    })),
  };
}

const body = (data) => Response.json(data);
const frame = (entry, cursor) => `${cursor ? 'id: ' + cursor + '\n' : ''}data: ${JSON.stringify(entry)}\n\n`;

test('keeps the exact steady workload, bounds cadence, and retains background traffic', () => {
  const tasks = loadPlan();
  const steady = tasks.filter(({ at }) => at >= steadyStart && at < steadyEnd);
  const turns = steady.filter(({ kind }) => kind === 'turn');
  assert.deepEqual(protocols.map((name) => turns.filter(({ protocol }) => protocol === name).length), [240, 180, 180]);
  assert.equal(steady.filter(({ kind }) => kind === 'read').length, 18_000);
  assert.equal(steady.filter(({ kind }) => kind === 'burst').length, 300);
  assert.equal(tasks.filter(({ kind, at }) => kind === 'connect' && at === 0).length, 25);
  assert.equal(tasks.filter(({ kind }) => kind === 'connect').length, 50);
  for (let user = 0; user < 50; user += 1) {
    const offsets = turns.filter((row) => row.user === user).map((row) => row.at);
    assert.equal(offsets.length, 12);
    assert.ok(offsets.slice(1).every((at, index) => at - offsets[index] === 300_000));
  }
  const delivery = [...deliveryPlan(config())];
  assert.equal(delivery.filter(({ kind }) => kind === 'background').length, 21_600);
  assert.ok(delivery.slice(1).every(({ at }, index) => at >= delivery[index].at));
});

test('rejects shared fixtures, missing provider coverage, and absent headroom evidence', () => {
  assert.equal(validateConfig(config()).users.length, 50);
  const invalid = config();
  invalid.users[1].bearer = invalid.users[0].bearer;
  assert.throws(() => validateConfig(invalid), /owner_fixture_reused/);
  const noFunded = config();
  for (const user of noFunded.users) user.providerClass = 'deterministic';
  assert.throws(() => validateConfig(noFunded), /funded_cohort_missing/);
  const noK = config();
  noK.reservation.verifiedK = 49;
  assert.throws(() => validateConfig(noK), /fixture_and_verified_capacity_required/);
});

test('parses frames split across transport chunks and rejects incomplete frames', async () => {
  const chunks = (lines) => (async function* () {
    for (const line of lines) yield new TextEncoder().encode(line);
  })();
  const rows = [];
  await consumeEvents(chunks(['id: 1\r', '\ndata: {"value":\n', 'data: 2}\r\n\r', '\n']), (row, id) => rows.push([row, id]));
  assert.deepEqual(rows, [[{ value: 2 }, '1']]);
  await assert.rejects(consumeEvents(chunks(['data: {"unfinished":true}\n']), () => {}), /stream_frame_incomplete/);
});

test('reconnects the observer from its last fully parsed cursor', async () => {
  const fixture = config().users[0];
  const abort = new AbortController();
  const urls = [];
  const records = [];
  const client = new CapacityClient(origin, fixture, {
    wait: async () => {},
    fetcher: async (url) => {
      urls.push(new URL(url));
      return new Response(frame({ agent_id: fixture.agent, incarnation: proof, cursor: cursor(4),
        kind: { type: 'turn_started' } }, cursor(4)), { headers: streamHeaders });
    },
  });
  await client.observe(cursor(1), abort.signal, (type, value) => {
    records.push([type, value]);
    if (type === 'stream_attempt' && urls.length === 1) abort.abort();
  });
  assert.deepEqual(urls.map((url) => url.searchParams.get('after_cursor')), [cursor(1), cursor(4)]);
  assert.equal(records.filter(([type]) => type === 'stream_cursor').length, 1);
});

for (const protocol of protocols) {
  test(`verifies durable response and independently hashes the output file for ${protocol}`, async () => {
    const fixture = config().users[0];
    const task = turnFixture('synthetic-run', 0, 0, fixture.delaySeconds, sha);
    const events = [];
    const paths = [];
    const client = new CapacityClient(origin, fixture, {
      now: () => 20,
      fetcher: async (url, request) => {
        const location = new URL(url);
        paths.push(location.pathname);
        assert.equal(request.redirect, 'error');
        if (request.method === 'POST') {
          assert.equal(request.headers['Idempotency-Key'], task.key);
          assert.match(request.body, /lc01-fixture.txt/);
          const headers = { ...streamHeaders, 'x-cma-command-id': 'cmd_example', 'x-cma-response-id': 'resp_example' };
          if (protocol === 'rest') {
            client.offerRestBinding({ kind: { type: 'turn_started' },
              response_binding: { command_id: 'cmd_example', response_id: 'resp_example' } });
            client.offerRestBinding({ kind: { type: 'message', role: 'user', content: task.input } });
            return Response.json({ queued: true }, { status: 202 });
          }
          const event = protocol === 'agui'
            ? { type: 'RUN_FINISHED', threadId: fixture.agent }
            : { type: 'response.completed', sequence_number: 8, response_id: 'resp_example' };
          return new Response(frame(event), { headers });
        }
        if (location.pathname.includes('/files/')) {
          assert.equal(location.searchParams.get('path'), task.outputPath);
          const content = Buffer.from(task.nonce + '\n');
          return body({ path: task.outputPath, size_bytes: content.length, sha256: task.expectedHash,
            content_base64: content.toString('base64') });
        }
        return body({ id: 'resp_example', status: 'completed', conversation: { id: fixture.agent } });
      },
    });
    const result = await client.turn({ run: 'synthetic-run', user: 0, at: 0, protocol, started: 0,
      fixtureSha: sha, record: (type, data) => events.push([type, data]) });
    const postPath = { rest: `/api/v1/agents/${fixture.agent}/input`, agui: '/api/v1/agui/run',
      responses: `/api/v2/agents/${fixture.agent}/responses` };
    assert.deepEqual(paths, [postPath[protocol], '/api/v2/responses/resp_example', `/api/v1/agents/${fixture.agent}/files/content`]);
    assert.equal(result, true);
    assert.deepEqual(events.at(-1), ['file_verified', { ok: true }]);
  });
}

test('keeps an ambiguous admission unresolved and performs no replay mutation', async () => {
  const events = [];
  const client = new CapacityClient(origin, config().users[0], {
    fetcher: async () => new Response(null, { status: 503 }),
  });
  const result = await client.turn({ run: 'synthetic-run', user: 0, at: 0, protocol: 'rest', started: 0,
    fixtureSha: sha, record: (type) => events.push(type) });
  assert.equal(result, false);
  assert.deepEqual(events, ['unknown']);
});

test('checks the served revision before opening any fixture or emitting the start record', async () => {
  const previous = globalThis.fetch;
  try {
    const requests = [];
    globalThis.fetch = async (url) => {
      requests.push(new URL(url).pathname);
      return body({ source_sha: 'c'.repeat(40) });
    };
    const rows = [];
    await assert.rejects(runCapacity(config(), (row) => rows.push(row), new AbortController().signal),
      /staging_build_changed/);
    assert.equal(rows.length, 1);
    assert.equal(rows[0].type, 'run');
    assert.ok(!JSON.stringify(rows).includes('synthetic-test-owner-'));
    assert.deepEqual(requests, Array(5).fill('/api/v1/build-info'));
  } finally {
    globalThis.fetch = previous;
  }
});

function capture(run = 'example') {
  const fixture = config();
  const rows = [{ type: 'run', run, ...fixture }, { type: 'start', utc: 'start' }];
  rows[0].reservation.manifestHash = workloadShape(fixture);
  const oracle = [];
  const expectedEventHashes = [];
  let fundedAdmitted = 0;
  for (const { at, user, kind, protocol } of deliveryPlan(fixture)) {
    if (kind === 'connect') {
      rows.push({ type: 'stream_attempt', user, cursor: 1, ms: at });
      rows.push({ type: 'stream_open', user, cursor: 1, ms: at + 1, headersMs: 1 });
      rows.push({ type: 'stream_cursor', user, cursor: 2, ms: at + 2, eventHash: proof });
      expectedEventHashes.push(hash(`${user}/2/${proof}`));
    } else if (kind === 'disconnect') {
      rows.push({ type: 'stream_close', user, cursor: 2, ms: at, reason: 'scheduled' });
    } else if (kind === 'turn') {
      const commandHash = hash(`${run}/command/${user}/${at}`);
      const responseHash = hash(`${run}/response/${user}/${at}`);
      const position = { user, at, protocol };
      rows.push({ type: 'offered', ...position }, { type: 'admitted', ...position, headersMs: 10,
        commandHash, responseHash, keyHash: hash(`${run}/key/${user}/${at}`) },
      { type: 'terminal', ...position, state: 'completed', responseHash, elapsedMs: 15_000 },
      { type: 'file_verified', ...position, ok: true });
      oracle.push({ commandHash, responseHash, nativeToPublicMs: 100, accepted: true, executions: 1,
        effects: 1, staleWrites: 0, terminal: true, correctOwner: true });
      if (at >= steadyStart && at < steadyEnd && fixture.users[user].providerClass === 'funded') fundedAdmitted += 1;
    } else {
      const consumer = kind === 'background' ? fixture.background[user].consumer : 'public';
      rows.push({ type: 'read', kind, user, at, consumer, http: 200, latencyMs: 50, failure: null });
    }
  }
  rows.push({ type: 'end', utc: 'end', ms: end, unresolvedOwners: 0 });
  return {
    rows,
    evidence: {
      run, startUtc: 'start', endUtc: 'end', sourceSha, imageDigest, cohortHash: proof, evidenceHash: proof,
      placementHash: proof, runtimeMappingHash: proof,
      oracle: { independent: true, complete: true, evidenceHash: proof, commands: oracle,
        acceptedHashes: oracle.map((row) => row.commandHash) },
      streams: { complete: true, cursorEvidenceHash: proof, expectedEventHashes },
      provider: { quotaEvidenceHash: proof, creditEvidenceHash: proof, latencyEvidenceHash: proof,
        healthyQuota: true, fundedAdmitted },
      faults: [], faultEvidenceHash: proof, cleanupComplete: true, cleanupEvidenceHash: proof, resourcesHash: proof,
      memberInventory: { run, startUtc: 'start', endUtc: 'end', sourceSha, imageDigest, independent: true,
        evidenceHash: proof, amReplicaHashes: [proof], mongoServerHashes: [proof], egressPoolHashes: [proof] },
      am: [{ replicaHash: proof, idleCpu: [0.2], steadyCpu: [0.4], burstCpu: [0.5], memory: [0.5], throttle: [0] }],
      mongo: { atlasHeadroom: [0.5, 0.5, 0.5], servers: [{ serverHash: proof, occupancy: [0.3],
        checkoutMs: Array(1000).fill(10) }] },
      nPlusOneHash: proof, egressPools: [{ poolHash: proof, cpuWithPending: [0.5], memoryWithPending: [0.5],
        largestNodeLossPlaceable: true }],
    },
  };
}

test('scores a full capture but blocks unqualified high-percentile strata', () => {
  const scored = scoreCapacity([capture()]);
  assert.equal(scored.status, 'BLOCKED');
  assert.deepEqual(scored.failures, []);
  assert.equal(scored.strata['read/public/read'].n, 18_000);
  assert.equal(scored.strata['read/public/read'].p99, 50);
  assert.deepEqual(distribution(Array(999).fill(50)), { n: 999, max: 50 });
  assert.equal(distribution(Array(1000).fill(50)).p99, 50);
});

test('rejects shifted observation windows and detects missing effects and sequence coverage', () => {
  const missing = capture();
  missing.evidence.oracle.commands[0].effects = 0;
  assert.throws(() => scoreCapacity([missing]), /accepted_effect_lost/);
  const shifted = capture();
  shifted.evidence.endUtc = 'other-window';
  assert.throws(() => scoreCapacity([shifted]), /evidence_run_candidate_or_window_mismatch/);
  const sequence = capture();
  sequence.evidence.streams.expectedEventHashes.pop();
  const score = scoreCapacity([sequence]);
  assert.equal(score.status, 'FAIL');
  assert.ok(score.failures.includes('stream_replay_or_sequence_gap'));
});

test('only combines supplemental captures from the same qualified cohort and deployment', () => {
  const first = capture('first');
  const supplemental = capture('supplemental');
  const stratum = 'read/public/burst';
  assert.equal(scoreCapacity([first]).strata[stratum].n, 300);
  assert.equal(scoreCapacity([first, supplemental]).strata[stratum].n, 600);
  for (const field of ['cohortHash', 'placementHash', 'runtimeMappingHash']) {
    const different = capture('different');
    const other = hash(field);
    different.rows[0].reservation[field] = other;
    different.evidence[field] = other;
    const score = scoreCapacity([different]);
    assert.equal(score.status, 'BLOCKED');
    assert.ok(score.insufficientSamples.includes(stratum));
    assert.throws(() => scoreCapacity([first, different]), /sample_shape_changed/);
  }
});

test('insufficient Mongo checkout samples do not hide missing measurements or observed failures', () => {
  const sample = capture();
  const [server] = sample.evidence.mongo.servers;
  server.checkoutMs = [10];
  const score = scoreCapacity([sample]);
  assert.equal(score.status, 'BLOCKED');
  assert.ok(score.insufficientSamples.includes(`mongo/checkout/${server.serverHash}`));
  server.checkoutMs = [1200];
  assert.equal(scoreCapacity([sample]).status, 'FAIL');
  delete server.occupancy;
  assert.throws(() => scoreCapacity([sample]), { code: 'infrastructure_samples_missing', blocked: false });
});

test('rejects an admission already counted under a different run ID', () => {
  const first = capture('first');
  const supplemental = capture('supplemental');
  const original = first.evidence.oracle.commands[0];
  const repeat = supplemental.evidence.oracle.commands[0];
  const row = supplemental.rows.find((entry) => entry.type === 'admitted' && entry.commandHash === repeat.commandHash);
  supplemental.evidence.oracle.acceptedHashes[0] = original.commandHash;
  repeat.commandHash = original.commandHash;
  row.commandHash = original.commandHash;
  assert.equal(scoreCapacity([supplemental]).status, 'BLOCKED');
  assert.throws(() => scoreCapacity([first, supplemental]), { code: 'admission_identity_reused', blocked: false });
});

test('pools Mongo checkout latency by server across comparable captures', () => {
  const first = capture('first');
  const second = capture('second');
  first.evidence.mongo.servers[0].checkoutMs = Array(600).fill(10);
  second.evidence.mongo.servers[0].checkoutMs = Array(600).fill(12);
  const stratum = `mongo/checkout/${proof}`;
  const combined = scoreCapacity([first, second]);
  assert.equal(combined.insufficientSamples.includes(stratum), false);
  assert.equal(combined.strata[stratum].n, 1200);
  assert.equal(combined.strata[stratum].max, 12);
  assert.equal(combined.strata[stratum].p99, 12);

  const other = hash('other server');
  second.evidence.mongo.servers[0].serverHash = other;
  second.evidence.memberInventory.mongoServerHashes = [other];
  const separate = scoreCapacity([first, second]);
  assert.deepEqual(separate.strata[stratum], { n: 600, max: 10 });
  assert.deepEqual(separate.strata[`mongo/checkout/${other}`], { n: 600, max: 12 });
  assert.ok(separate.insufficientSamples.includes(stratum));
  assert.ok(separate.insufficientSamples.includes(`mongo/checkout/${other}`));
});

test('exports failed JSON and JUnit for completed runs with unknown telemetry or provider credit', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'cma-lc01-score-'));
  t.after(() => rm(directory, { recursive: true }));
  const sample = capture();
  const cli = fileURLToPath(new URL('../lc01.mjs', import.meta.url));
  const raw = join(directory, 'capture.ndjson');
  await writeFile(raw, sample.rows.map((row) => JSON.stringify(row)).join('\n'));
  const { am, ...withoutAM } = sample.evidence;
  assert.ok(am.length > 0);
  const { creditEvidenceHash, ...withoutCredit } = sample.evidence.provider;
  assert.ok(creditEvidenceHash);
  const fixtures = [
    ['resource_evidence_missing', withoutAM],
    ['funded_provider_evidence_missing', { ...sample.evidence, provider: withoutCredit }],
    ['am_replica_inventory_mismatch', { ...sample.evidence,
      memberInventory: { ...sample.evidence.memberInventory, amReplicaHashes: [proof, hash('missing replica')] } }],
    ['mongo_server_inventory_mismatch', { ...sample.evidence,
      memberInventory: { ...sample.evidence.memberInventory, mongoServerHashes: [proof, hash('missing server')] } }],
    ['egress_pool_inventory_mismatch', { ...sample.evidence,
      memberInventory: { ...sample.evidence.memberInventory, egressPoolHashes: [proof, hash('missing pool')] } }],
  ];
  for (const [code, evidence] of fixtures) {
    const proofFile = join(directory, `${code}.observations.json`);
    const report = join(directory, `${code}.json`);
    const junit = join(directory, `${code}.xml`);
    await writeFile(proofFile, JSON.stringify(evidence));
    const run = spawnSync(process.execPath, [cli, 'score', report, junit, raw, proofFile], { encoding: 'utf8' });
    assert.equal(run.status, 2, run.stderr);
    assert.equal(JSON.parse(await readFile(report, 'utf8')).status, 'FAIL');
    assert.equal(JSON.parse(await readFile(report, 'utf8')).code, code);
    const xml = await readFile(junit, 'utf8');
    assert.match(xml, /failures="1" skipped="0"/);
    assert.match(xml, /<failure /);
  }
});
