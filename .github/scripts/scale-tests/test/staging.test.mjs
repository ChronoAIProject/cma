import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { validateConfig, validateAuthority, stagingEnvironment, fixtureName } from '../staging/authority.mjs';
import { prepareWire } from '../staging/prepare.mjs';
import { safeRow, Evidence } from '../staging/evidence.mjs';
import { localUrl } from '../lib/safety.mjs';
import { config, authority, fakeRun } from './staging-fixtures.mjs';

const directory = await mkdtemp(join(tmpdir(), 'staging-driver-test-'));
const parserPath = join(directory, 'wire.mjs');
await prepareWire(parserPath);
const wire = await import(pathToFileURL(parserPath).href);
after(() => rm(directory, { recursive: true, force: true }));

test('separate staging authority refuses missing profile and preserves local-only guard', () => {
  assert.throws(() => validateConfig(config(), {}), /staging_profile_required/);
  assert.throws(() => validateConfig(config(), { NYXID_PROFILE: 'production' }), /staging_profile_required/);
  assert.deepEqual(stagingEnvironment({ NYXID_PROFILE: 'staging', NYXID_URL: 'foreign', NYXID_ACCESS_TOKEN: 'synthetic', PATH: 'bin' }),
    { NYXID_PROFILE: 'staging', PATH: 'bin' });
  assert.throws(() => localUrl(config().origin));
  assert.equal(validateConfig(config(), { NYXID_PROFILE: 'staging' }).profile, 'staging');
});
test('authority binds actual service, candidate, subject, freshness and stop state', () => {
  const c = config();
  for (const change of [{ profile: 'production' }, { origin: 'https://foreign.invalid' }, { serviceMapHash: 'wrong' },
    { candidateHash: 'wrong' }, { subjectHashes: [] }, { observedAt: '2020-01-01' }, { stop: true },
    { providerReady: false }, { capacityHomes: 0 }]) assert.throws(() => validateAuthority(c, { ...authority(c), ...change }));
  assert.doesNotThrow(() => validateAuthority(c, { ...authority(c), stop: true }, { cleanup: true }));
});
test('fixture authority rejects duplicates, absent enforcement and insufficient funding', () => {
  for (const mutate of [(c) => c.fixtures.push(c.fixtures[0]), (c) => { c.policy.toolsDisabled = false; },
    (c) => { c.modelCap = 0; }, (c) => { c.fixtures[0].browserSample = false; }]) {
    const c = config(); mutate(c); assert.throws(() => validateConfig(c, { NYXID_PROFILE: 'staging' }));
  }
});
test('real parser + independent transcript complete one REST input and exact fixture cleanup', async () => {
  const { run, http, rows } = fakeRun(wire);
  const result = await run.scope(async (driver) => {
    const fixture = await driver.create(1);
    const first = fixture.turn('offer_1');
    assert.equal(fixture.turn('offer_1'), first);
    assert.equal((await first).completed, true);
  });
  assert.equal(result.completed, 1); assert.equal(result.admitted, 1);
  assert.equal(result.logicalCleanup, true); assert.equal(result.cleanup, false);
  const creates = http.calls.filter((c) => c.method === 'POST' && !c.path.endsWith('/input') && !c.path.endsWith('/stop'));
  assert.equal(creates.length, 3);
  assert.equal(JSON.parse(creates[0].body).name, fixtureName(1));
  assert.equal(JSON.parse(creates[2].body).title, fixtureName(1));
  assert.equal(Object.hasOwn(JSON.parse(creates[2].body), 'first_message'), false);
  assert.equal(http.calls.filter((c) => c.path.endsWith('/input')).length, 1);
  assert.ok(http.calls.some((c) => c.path.includes('/transcript')));
  assert.ok(rows.some((r) => r.status === 'PASS' && r.scenario === 'rest_turn'));
  assert.ok(!JSON.stringify(rows).includes('agt_synthetic'));
  assert.ok(!JSON.stringify(rows).includes('Only reply'));
});
for (const failure of ['sandbox', 'workspace', 'agent']) test(`partial ${failure} setup retains uncertainty and cleans only exact owned parent`, async () => {
  const { run, http, rows } = fakeRun(wire, { failCreate: failure });
  const result = await run.scope(async (driver) => { await driver.create(1); });
  assert.equal(result.cleanup, false); assert.equal(result.logicalCleanup, false);
  assert.ok(rows.some((r) => r.code === 'custody_unresolved'));
  const deletes = http.calls.filter((c) => c.method === 'DELETE');
  assert.equal(deletes.some((c) => c.path === '/api/v1/sandboxes/sbx_synthetic'), failure !== 'sandbox');
  assert.equal(http.calls.filter((c) => c.path === '/api/v1/sandboxes' && c.method === 'POST').length, 1);
});
test('wrong owner is never adopted or deleted; browser failure still retires owned hierarchy', async () => {
  for (const options of [{ wrongOwner: true }, { browserFails: true }]) {
    const { run, http } = fakeRun(wire, options);
    const result = await run.scope(async (driver) => { await driver.create(1); });
    assert.equal(result.cleanup, false);
    assert.equal(http.calls.some((c) => c.method === 'DELETE'), !options.wrongOwner);
  }
});
for (const mode of ['wrong_nonce', 'wrong_agent', 'gap', 'missing_binding', 'failed', 'echo_only'])
  test(`queued ACK and ${mode} never qualify as completed sample`, async () => {
    const { run, rows } = fakeRun(wire, { mode });
    await run.scope(async (driver) => { const f = await driver.create(1); await f.turn('offer'); });
    assert.equal(rows.some((r) => r.scenario === 'rest_turn' && r.status === 'PASS'), false);
    assert.ok(run.stopped);
    if (mode === 'echo_only') assert.ok(rows.some((r) => r.overdue === 1));
  });
test('independent history disagreement fails; lost input ACK retains original key and settles without resend', async () => {
  for (const options of [{ badDurable: true }, { lostAck: true }]) {
    const { run, rows, http } = fakeRun(wire, options);
    const result = await run.scope(async (driver) => { const f = await driver.create(1); await f.turn('offer'); });
    assert.equal(rows.some((r) => r.scenario === 'rest_turn' && r.status === 'PASS'), false);
    assert.equal(http.calls.filter((c) => c.path.endsWith('/input')).length, 1);
    if (options.lostAck) { assert.equal(result.completed, 0); assert.equal(result.settledTurns, 1); assert.equal(result.admitted, 0); }
  }
});
test('one primary observation body stays open across successive turns and closes after settlement', async () => {
  const { run, http } = fakeRun(wire);
  const result = await run.scope(async (driver) => {
    const fixture = await driver.create(1);
    await fixture.turn('first');
    assert.equal(http.http.streamOpened, 1); assert.equal(http.http.streamClosed, 0);
    await fixture.turn('second');
    await fixture.turn('third');
  });
  assert.equal(result.completed, 3);
  const pages = http.calls.filter((c) => c.path.includes('/transcript'));
  assert.equal(pages.length, 6);
  assert.ok(pages.some((c) => c.path.includes('turn=10&after=9&through=12&limit=32')));
  assert.equal(http.http.streamOpened, 1); assert.equal(http.http.streamClosed, 1);
});
test('stop forbids offers while cleanup remains authorized', async () => {
  const { run, http } = fakeRun(wire);
  await run.scope(async (driver) => {
    const fixture = await driver.create(1); driver.stop();
    assert.throws(() => fixture.turn('later'), /agent_turn_unavailable/);
  });
  assert.equal(http.calls.some((c) => c.path.endsWith('/input')), false);
  assert.equal(http.calls.some((c) => c.method === 'DELETE'), true);
});
test('failed Agent stop still retires its owned Sandbox and independently checks the Agent', async () => {
  const { run, http, rows } = fakeRun(wire, { stopFails: true });
  const result = await run.scope(async (driver) => { await driver.create(1); });
  assert.ok(rows.some((r) => r.scenario === 'cleanup_agent' && r.status === 'FAIL'));
  assert.ok(http.calls.some((c) => c.method === 'DELETE' && c.path.includes('/sandboxes/')));
  assert.equal(result.logicalCleanup, true); assert.equal(result.cleanup, false);
});
test('concurrent offers cannot both reserve the final funded turn or bypass a stop during authority read', async () => {
  const { run } = fakeRun(wire);
  run.config.maxTurns = 1;
  const results = await Promise.allSettled([run.admit(run.config.fixtures[0], 'turn', 1000),
    run.admit(run.config.fixtures[0], 'turn', 1000)]);
  assert.equal(results.filter((r) => r.status === 'fulfilled').length, 1);
  assert.equal(run.offered, 1);
  const other = fakeRun(wire).run;
  other.readAuthority = async () => { other.stop(); return authority(other.config); };
  await assert.rejects(other.admit(other.config.fixtures[0], 'turn', 1000), /admission_stopped/);
});
test('evidence write failure closes admission but still finalizes previously owned resources', async () => {
  const { run, http } = fakeRun(wire);
  const record = run.evidence.record;
  run.evidence.record = (row) => {
    if (row.scenario === 'fixture_ready' || run.evidenceFailed) throw new Error('synthetic full disk');
    return record(row);
  };
  await run.scope(async (driver) => { await driver.create(1); });
  assert.ok(run.evidenceFailed);
  assert.ok(http.calls.some((c) => c.method === 'DELETE' && c.path.includes('/sandboxes/')));
});
for (const kind of ['sandbox', 'workspace', 'agent']) for (const permanent of [false, true])
  test(`${kind} custody survives ${permanent ? 'permanent' : 'one-shot'} acknowledgement journal failure`, async () => {
    const { run, http, rows } = fakeRun(wire);
    const record = run.evidence.record; let failed = false;
    run.evidence.record = (row) => {
      if ((!failed && row.scenario === `create_${kind}` && row.status === 'acknowledged') || (permanent && failed)) {
        failed = true; throw new Error('synthetic acknowledgement fsync failure');
      }
      return record(row);
    };
    const result = await run.scope(async (driver) => { await driver.create(1); });
    const fixture = run.fixtures.get(1);
    assert.ok(failed); assert.ok(run.stopped); assert.ok(run.evidenceFailed);
    assert.ok(fixture[kind]); assert.ok(fixture.intents.get(`create_${kind}`).verified);
    assert.equal(fixture.intents.get(`create_${kind}`).state, 'acknowledged');
    assert.equal(result.logicalCleanup, true); assert.equal(result.cleanup, false);
    assert.ok(http.calls.some((c) => c.method === 'DELETE' && c.path === '/api/v1/sandboxes/sbx_synthetic'));
    assert.equal(http.calls.some((c) => c.method === 'DELETE' && c.path === '/api/v1/agents/agt_synthetic'), kind === 'agent');
    const creates = http.calls.filter((c) => c.method === 'POST' && !c.path.endsWith('/stop'));
    assert.equal(creates.length, ['sandbox', 'workspace', 'agent'].indexOf(kind) + 1);
    assert.equal(new Set(creates.map((c) => c.key)).size, creates.length);
    assert.ok(!JSON.stringify(rows).includes('sbx_synthetic'));
  });
for (const kind of ['sandbox', 'workspace', 'agent']) test(`foreign ${kind} receipt never enters cleanup custody`, async () => {
  const { run, http } = fakeRun(wire);
  const request = http.request.bind(http);
  http.request = async (path, options = {}) => {
    const result = await request(path, options);
    if (options.method === 'POST' && result.status === 201) {
      if (kind === 'sandbox' && path === '/api/v1/sandboxes') result.value.owner = 'foreign-owner';
      if (kind === 'workspace' && path.endsWith('/workspaces/inline')) result.value.sandbox_id = 'sbx_foreign';
      if (kind === 'agent' && path.endsWith('/agents')) result.value.workspace.id = 'wsp_foreign';
    }
    return result;
  };
  const result = await run.scope(async (driver) => { await driver.create(1); });
  assert.equal(run.fixtures.get(1)[kind], undefined); assert.equal(result.logicalCleanup, false);
  assert.equal(http.calls.some((c) => c.method === 'DELETE' && c.path.includes('/agents/')), false);
  assert.equal(http.calls.some((c) => c.method === 'DELETE' && c.path.includes('/sandboxes/')), kind !== 'sandbox');
});
test('all creation routes share subject quota and no second setup can overlap', async () => {
  const { run, time } = fakeRun(wire);
  const spec = run.config.fixtures[0]; spec.createPerMinute = 5;
  for (let i = 0; i < 5; i++) await run.quota(spec, 'create', 120000);
  assert.equal(time.now(), 60000);
  const first = run.create(1);
  await assert.rejects(run.create(1), /setup_stopped_or_in_progress/);
  await assert.rejects(first);
  await run.close();
});
test('evidence rejects raw identifiers and reports staging without qualification claims', async () => {
  assert.throws(() => safeRow({ agentId: 'private' }), /unapproved_evidence_field/);
  const evidence = await Evidence.open(join(directory, 'evidence'));
  evidence.record({ scenario: 'synthetic_check', status: 'PASS', elapsedMs: 1 });
  const report = await evidence.finish({ cleanup: false });
  assert.equal(report.environment, 'staging'); assert.equal(report.status, 'BLOCKED');
  assert.equal(JSON.parse(await readFile(join(directory, 'evidence/manifest.json'))).files.length, 2);
});
