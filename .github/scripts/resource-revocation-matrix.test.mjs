import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, truncateSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { checkReleaseMatrix, main, releaseRoutes } from './resource-revocation-contracts.mjs';
import { binding, hash, observation, schedules } from './resource-revocation-contracts.fixture.mjs';

const { route: unusedRoute, ...common } = binding;
const routes = releaseRoutes();
const script = fileURLToPath(new URL('./resource-revocation-contracts.mjs', import.meta.url));
function campaign(route, origin = 'candidate', selected = schedules) {
  return {
    route,
    observations: selected.map(schedule => {
      const row = observation(schedule, origin);
      row.binding.route = route;
      row.identity.release = hash(route + row.identity.release);
      return row;
    }),
  };
}
function failed(route = routes[0]) {
  const delayed = schedules.find(row => row.actions.indexOf('effective_commit') < row.actions.indexOf('chunk_0_handoff_attempt'));
  const entry = campaign(route, 'candidate', [delayed]);
  entry.observations[0].events.at(-1).private_bytes_total++;
  return entry;
}

test('matrix plan preserves the declared route inventory and remains unexecuted', () => {
  const result = main(['matrix-plan']);
  assert.equal(result.exit, 2);
  assert.deepEqual(result.report.routes, routes.map(route => ({ route, execution: 'not_executed' })));
  assert.equal(routes.length, 7);
  assert.equal(result.report.qualification, 'not_assessed');
  assert.equal(result.report.activation_authorized, false);
  releaseRoutes().pop();
  assert.deepEqual(releaseRoutes(), routes);
});

test('seven raw candidate-shaped campaigns pass without asserting qualification', () => {
  const report = checkReleaseMatrix(common, routes.map(route => campaign(route)));
  assert.equal(report.status, 'pass');
  assert.deepEqual(report.errors, []);
  assert.deepEqual(report.missing_routes, []);
  assert.equal(report.campaigns.length, 7);
  assert(report.campaigns.every(row => row.report.distinct_candidate_schedules_passed === 1716));
  assert.equal(report.qualification, 'not_assessed');
  assert.equal(report.activation_authorized, false);
  // Shared operation/grant/API-boot identities across routes are legitimate;
  // these fabricated inputs still represent zero actual candidate executions.
});

test('synthetic matrices and prior PASS summaries cannot replace raw candidate traces', () => {
  const synthetic = checkReleaseMatrix(common, routes.map(route => campaign(route, 'synthetic')));
  assert.equal(synthetic.status, 'blocked');
  assert(synthetic.campaigns.every(row => row.report.distinct_candidate_schedules_passed === 0));
  const summaries = routes.map(route => ({ route, observations: { status: 'pass', distinct_candidate_schedules_passed: 1716 } }));
  assert.equal(checkReleaseMatrix(common, summaries).status, 'blocked');
});

test('a complete matrix cannot pool another route counts to repair 999 schedules', () => {
  const entries = routes.map(route => campaign(route));
  entries[3].observations = entries[3].observations.slice(0, 999);
  const report = checkReleaseMatrix(common, entries);
  assert.equal(report.status, 'blocked');
  assert(report.campaigns[3].report.errors.includes('fewer_than_1000_distinct_candidate_schedules'));
  assert(report.campaigns.filter(row => row.status === 'pass').length === 6);
});

for (const [reason, predicate] of [
  ['missing_shared_positive_schedule', a => a.indexOf('chunk_1_handoff_attempt') < a.indexOf('revoke_request')],
  ['missing_pause_before_first_chunk', a => a.indexOf('final_check_attempt') < a.indexOf('revoke_request') && a.indexOf('effective_commit') < a.indexOf('chunk_0_handoff_attempt')],
  ['missing_pause_between_chunks', a => a.indexOf('chunk_0_handoff_attempt') < a.indexOf('effective_commit') && a.indexOf('effective_commit') < a.indexOf('chunk_1_handoff_attempt')],
]) test(`matrix retains the per-route ${reason} refusal`, () => {
  const selected = schedules.filter(row => !predicate(row.actions));
  const report = checkReleaseMatrix(common, [campaign(routes[0], 'candidate', selected)]);
  assert.equal(report.status, 'blocked');
  assert(report.campaigns[0].report.errors.includes(reason));
});

test('missing, unknown and duplicate routes never masquerade as complete coverage', () => {
  const missing = checkReleaseMatrix(common, routes.slice(1).map(route => campaign(route)));
  assert.equal(missing.status, 'blocked');
  assert.deepEqual(missing.missing_routes, [routes[0]]);
  const repeated = checkReleaseMatrix(common, Array.from({ length: 7 }, () => campaign(routes[0])));
  assert.equal(repeated.status, 'blocked');
  assert(repeated.errors.includes('duplicate_route'));
  assert.equal(repeated.missing_routes.length, 6);
  let loads = 0;
  const unknown = checkReleaseMatrix(common, [{ route: 'private-unrecognized-fixture' }], () => loads++);
  assert.equal(loads, 0);
  assert.equal(unknown.status, 'blocked');
  assert(!JSON.stringify(unknown).includes('private-unrecognized-fixture'));
});

test('cross-route release reuse blocks while ordinary shared operation identities remain allowed', () => {
  const entries = routes.slice(0, 2).map(route => campaign(route));
  entries[1].observations[0].identity.release = entries[0].observations[0].identity.release;
  const report = checkReleaseMatrix(common, entries);
  assert.equal(report.campaigns[0].status, 'pass');
  assert.equal(report.campaigns[1].status, 'blocked');
  assert.deepEqual(report.campaigns[1].errors, ['release_identity_reused_across_routes']);
});

test('route binding and each common candidate field remain exact', () => {
  for (const key of Object.keys(binding)) {
    const entry = campaign(routes[0], 'candidate', [schedules[0]]);
    entry.observations[0].binding[key] = key === 'route' ? routes[1] : key === 'source_revision' ? 'b'.repeat(40) : hash('other');
    const report = checkReleaseMatrix(common, [entry]);
    assert.equal(report.status, 'blocked');
    assert.deepEqual(report.campaigns[0].report.attempts[0].reasons, ['candidate_binding_mismatch']);
  }
  for (const invalid of [null, binding, { ...common, extra: true }, { ...common, source_revision: 'main' }])
    assert(checkReleaseMatrix(invalid, []).errors.includes('invalid_expected_binding'));
});

test('failed duplicates survive both ordering directions and loading failures', () => {
  for (const entries of [[failed(), campaign(routes[0])], [campaign(routes[0]), failed()]]) {
    const report = checkReleaseMatrix(common, entries);
    assert.equal(report.status, 'fail');
    assert.equal(report.campaigns.length, 2);
    assert(report.errors.includes('duplicate_route'));
  }
  for (const entries of [[failed(), { route: routes[1] }], [{ route: routes[1] }, failed()]]) {
    const report = checkReleaseMatrix(common, entries, entry => {
      if (!entry.observations) throw new Error('private-path-or-payload');
      return entry.observations;
    });
    assert.equal(report.status, 'fail');
    assert(report.campaigns.some(row => row.errors.includes('invalid_route_input')));
    assert(!JSON.stringify(report).includes('private-path-or-payload'));
  }
});

test('oversized inventories retain an admitted failure and cap loaded entries', () => {
  let loads = 0;
  const entries = [failed(), ...Array.from({ length: 100 }, () => ({ route: routes[1], observations: [] }))];
  const report = checkReleaseMatrix(common, entries, entry => { loads++; return entry.observations; });
  assert.equal(loads, 14);
  assert.equal(report.status, 'fail');
  assert(report.errors.includes('oversized_route_inventory'));
  assert.equal(checkReleaseMatrix(common, {}).status, 'blocked');
});

function files(run) {
  const folder = mkdtempSync(join(tmpdir(), 'cma-release-matrix-'));
  try {
    const expected = join(folder, 'expected.json');
    const manifest = join(folder, 'routes.json');
    writeFileSync(expected, JSON.stringify(common));
    const save = (entry, filename) => {
      const raw = JSON.stringify(entry.observations);
      writeFileSync(join(folder, filename), raw);
      return { route: entry.route, observations_file: filename, observations_sha256: hash(raw) };
    };
    const invoke = entries => {
      writeFileSync(manifest, JSON.stringify({ schema: 1, routes: entries }));
      return spawnSync(process.execPath, [script, 'check-matrix', expected, manifest], {
        encoding: 'utf8', timeout: 30_000, maxBuffer: 32 * 1024 * 1024, cwd: tmpdir(),
      });
    };
    run({ folder, expected, manifest, save, invoke });
  } finally { rmSync(folder, { recursive: true, force: true }); }
}

test('matrix CLI reads manifest-relative raw files and checks their exact bytes', () => files(({ save, invoke }) => {
  const entries = routes.map((route, index) => save(campaign(route), `route-${index}.json`));
  const result = invoke(entries);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).status, 'pass');
  entries[0].observations_sha256 = hash('different-bytes');
  const mismatch = invoke(entries);
  assert.equal(mismatch.status, 2, mismatch.stderr);
  assert.equal(JSON.parse(mismatch.stdout).campaigns[0].status, 'blocked');
}));

for (const mode of ['missing', 'malformed', 'oversized', 'directory', 'fifo', 'hash', 'summary', 'extra'])
  test(`matrix CLI retains actual trace failure beside ${mode} input`, () => files(({ folder, save, invoke }) => {
    const bad = save(failed(), 'failed.json');
    const file = 'private-fixture-input';
    const path = join(folder, file);
    let bytes = 'synthetic-private-payload';
    if (mode === 'summary') bytes = JSON.stringify({ status: 'pass', distinct_candidate_schedules_passed: 1716 });
    if (mode === 'directory') mkdirSync(path);
    else if (mode === 'fifo') assert.equal(spawnSync('mkfifo', [path]).status, 0);
    else if (mode !== 'missing') {
      writeFileSync(path, bytes);
      if (mode === 'oversized') truncateSync(path, 32 * 1024 * 1024 + 1);
    }
    const other = { route: routes[1], observations_file: file, observations_sha256: hash(bytes) };
    if (mode === 'hash') other.observations_sha256 = hash('wrong-bytes');
    if (mode === 'extra') other.extra = true;
    for (const entries of [[bad, other], [other, bad]]) {
      const result = invoke(entries);
      assert.equal(result.status, 1, result.stderr);
      const report = JSON.parse(result.stdout);
      assert.equal(report.status, 'fail');
      assert(report.campaigns.some(row => row.report?.failed_attempts === 1));
      assert(!result.stdout.includes(file));
      assert(!result.stdout.includes('synthetic-private-payload'));
    }
  }));

test('the selected repository suite imports the matrix regressions', () => {
  const selected = readFileSync(new URL('./resource-revocation-contracts.test.mjs', import.meta.url), 'utf8');
  assert(selected.includes("import './resource-revocation-matrix.test.mjs'"));
});
