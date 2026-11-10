import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { sha256, validatePlan, validateEvidence, verifyFiles } from './migration-matrix.mjs';

const executable = role => ({ source: sha256(role).slice(0, 40),
  image: `ghcr.io/chronoaiproject/cma-api@sha256:${sha256(`${role}-image`)}` });
const start = '2026-11-10T14:00:00.000Z', end = '2026-11-10T14:01:00.000Z';

function campaign() {
  const candidate = executable('candidate');
  const readers = ['historical', 'current', 'candidate', 'recovery'].map(role => ({
    id: role, role, artifact: executable(role),
  }));
  const fixtures = ['retained', 'disabled', 'enabled'].map(policy => ({
    id: policy, sha256: sha256(`${policy}-fixture`), policy,
    writer: policy === 'retained' ? executable('historical') : candidate,
  }));
  const cells = readers.flatMap(reader => fixtures.map(fixture => ({
    id: `${reader.id}-${fixture.id}`, reader: reader.id, fixture: fixture.id,
    expectation: reader.role === 'historical' && fixture.policy === 'enabled' ? 'refuse' : 'accept',
  })));
  const plan = { schema: 'cma-migration-matrix-plan-v1', candidate, readers, fixtures, cells };
  const raw = new Map();
  const evidence = { schema: 'cma-migration-matrix-evidence-v1', plan_sha256: sha256(JSON.stringify(plan)),
    candidate, environment: 'isolated-fixture', configuration_sha256: sha256('fixture-configuration'),
    started_at: start, finished_at: end, cells: cells.map(cell => {
      const fixture = fixtures.find(row => row.id === cell.fixture);
      const row = { id: cell.id, reader: readers.find(row => row.id === cell.reader).artifact,
        writer: fixture.writer, fixture_sha256: fixture.sha256, policy: fixture.policy,
        execution: 'artifact', outcome: cell.expectation, attempt: 1, samples: 3,
        started_at: start, finished_at: end, checks: cell.expectation === 'accept'
          ? { decode: true, replay: true, restart: true, takeover: true, identity_preserved: true, writer_policy: true }
          : { refused_before_traffic: true, refused_before_write: true, identity_preserved: true } };
      const bytes = Buffer.from(JSON.stringify(row)), path = `${cell.id}.json`;
      raw.set(path, bytes);
      return { ...row, raw: { path, sha256: sha256(bytes) } };
    }) };
  return { plan, evidence, raw };
}

function validate(value) {
  return validateEvidence(value.plan, sha256(JSON.stringify(value.plan)), value.evidence, path => value.raw.get(path));
}

test('complete matrix binds artifact identities and excludes an incompatible predecessor', () => {
  const value = campaign();
  assert.deepEqual(validate(value), { cells: 12, accepted: 11, refused: 1 });
  // A compatible forward-repair image may intentionally equal the candidate.
  value.plan.readers[3].artifact = structuredClone(value.plan.candidate);
  value.evidence.plan_sha256 = sha256(JSON.stringify(value.plan));
  for (const row of value.evidence.cells.filter(row => row.id.startsWith('recovery-'))) {
    row.reader = value.plan.candidate;
    const { raw, ...observation } = row;
    const bytes = Buffer.from(JSON.stringify(observation));
    value.raw.set(raw.path, bytes);
    raw.sha256 = sha256(bytes);
  }
  assert.equal(validate(value).accepted, 11);
});

test('reviewed plans refuse missing roles, policy coverage, cells and recovery exclusions', () => {
  const mutations = [
    p => p.cells.pop(),
    p => { p.cells[1] = structuredClone(p.cells[0]); },
    p => { p.cells[1].reader = p.cells[0].reader; p.cells[1].fixture = p.cells[0].fixture; },
    p => { p.readers[0].role = 'current'; },
    p => { p.readers[3].role = 'historical'; },
    p => { p.fixtures[1].policy = 'retained'; },
    p => { p.fixtures[1].writer = executable('historical'); },
    p => { p.cells[9].expectation = 'refuse'; },
    p => { p.cells[6].expectation = 'refuse'; },
    p => { p.cells[0].reader = 'unlisted'; },
    p => { p.cells[0].expectation = 'skipped'; },
    p => { p.readers[2].artifact = executable('other'); },
    p => { p.readers[0].artifact.image = 'ghcr.io/chronoaiproject/cma-api:old'; },
    p => { p.readers[0].artifact.source = 'develop'; },
    p => { p.readers[0].artifact.image = p.readers[0].artifact.image.replace('cma-api@', 'cma-cmaeg@'); },
    p => { p.fixtures[0].writer.image = p.fixtures[0].writer.image.replace('cma-api@', 'cma-runtime@'); },
    p => { p.readers[0].artifact.image = `sha256:${'a'.repeat(64)}`; },
    p => { p.fixtures[0].sha256 = ''; },
    p => { p.unreviewed = true; },
  ];
  for (const mutate of mutations) {
    const { plan } = campaign();
    mutate(plan);
    assert.throws(() => validatePlan(plan));
  }
});

test('evidence cannot hide retries, skipped cells, wrong artifacts or lost recovery semantics', () => {
  const mutations = [
    e => e.cells.pop(),
    e => { e.cells[1] = structuredClone(e.cells[0]); },
    e => { e.cells[0].id = 'unreviewed-cell'; },
    e => { e.plan_sha256 = sha256('other-plan'); },
    e => { e.candidate = executable('other'); },
    e => { e.environment = 'unknown'; },
    e => { e.configuration_sha256 = ''; },
    e => { e.cells[0].execution = 'decoder-model'; },
    e => { e.cells[0].reader = executable('other'); },
    e => { e.cells[0].writer = executable('other'); },
    e => { e.cells[0].fixture_sha256 = sha256('other-fixture'); },
    e => { e.cells[0].policy = 'enabled'; },
    e => { e.cells[0].attempt = 2; },
    e => { e.cells[0].outcome = 'cancelled'; },
    e => { e.cells[0].samples = 0; },
    e => { e.cells[0].samples = 1.5; },
    e => { e.cells[0].samples = Number.MAX_SAFE_INTEGER + 1; },
    e => { e.cells[0].checks.restart = false; },
    e => { delete e.cells[0].checks.takeover; },
    e => { e.cells[0].checks.writer_policy = false; },
    e => { e.cells[2].checks.refused_before_write = false; },
    e => { e.cells[0].raw.sha256 = sha256('tampered'); },
    e => { e.cells[0].raw.path = 'missing.json'; },
    e => { e.cells[0].started_at = '2026-11-10T13:59:00.000Z'; },
    e => { e.cells[0].finished_at = '2026-11-10T14:02:00.000Z'; },
    e => { e.finished_at = '2026-02-30T14:01:00.000Z'; },
    e => { e.finished_at = start; e.started_at = end; },
    e => { e.cells[0].raw_diagnostics = 'unreviewed'; },
  ];
  for (const mutate of mutations) {
    const value = campaign();
    mutate(value.evidence);
    assert.throws(() => validate(value));
  }
});

test('raw receipts must exist and match the complete observation, even with a matching hash', () => {
  const value = campaign();
  const row = value.evidence.cells[0];
  const bytes = Buffer.from(JSON.stringify({ result: 'pass' }));
  value.raw.set(row.raw.path, bytes);
  row.raw.sha256 = sha256(bytes);
  assert.throws(() => validate(value));
});

test('CLI binds the independently selected candidate and withholds malformed raw diagnostics', () => {
  const root = mkdtempSync(join(tmpdir(), 'cma-migration-matrix-'));
  const script = new URL('./migration-matrix.mjs', import.meta.url);
  try {
    const value = campaign();
    const plan = join(root, 'plan.json'), evidence = join(root, 'evidence.json');
    writeFileSync(plan, JSON.stringify(value.plan));
    writeFileSync(evidence, JSON.stringify(value.evidence));
    for (const [path, bytes] of value.raw) writeFileSync(join(root, path), bytes);
    const args = [plan, evidence, value.plan.candidate.source, value.plan.candidate.image];
    assert.equal(verifyFiles(...args).cells, 12);
    assert.match(execFileSync(process.execPath, [script.pathname, ...args], { encoding: 'utf8' }), /12 cells/);
    assert.throws(() => verifyFiles(plan, evidence, executable('other').source, value.plan.candidate.image));
    const original = value.evidence.cells[0].raw.path;
    for (const path of ['/etc/passwd', '../escape.json', 'nested/../../escape', 'file\\escape', 'link.json']) {
      value.evidence.cells[0].raw.path = path;
      if (path === 'link.json') symlinkSync(join(root, original), join(root, path));
      writeFileSync(evidence, JSON.stringify(value.evidence));
      assert.throws(() => verifyFiles(...args));
    }
    writeFileSync(evidence, '{untrusted raw diagnostic');
    const result = spawnSync(process.execPath, [script.pathname, ...args], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, 'Migration matrix evidence is incomplete or inconsistent; raw diagnostics withheld\n');
    assert.equal(spawnSync(process.execPath, [script.pathname], { encoding: 'utf8' }).status, 1);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
