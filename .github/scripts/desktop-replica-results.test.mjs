import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdirSync, writeFileSync, readFileSync, copyFileSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {cleanupOutput, fixture, measurement, measurementOutput, parentOutput, parents, report} from './desktop-replica-fixture.test-helpers.mjs';

function check(options, expected, pattern) {
  const f = fixture(options);
  try {
    const result = f.run();
    const receipt = JSON.parse(f.read('receipt.json'));
    assert.equal(result.status, expected, result.stdout + result.stderr + JSON.stringify(receipt));
    if (pattern) assert.match(receipt.errors.join(' '), pattern);
    assert.equal(receipt.qualification.performance, 'not_evaluated');
    assert.equal(receipt.retries, 0);
    assert.equal(receipt.first_attempt_exit_code, options.fault === 'exit' ? 42 : 0);
    assert.ok(f.read('contracts.log').length);
    return receipt;
  } finally { f.close(); }
}

for (const mode of ['contracts', 'one', 'two']) {
  test(`${mode} validates exact first-attempt results and executed artifacts`, () => {
    const receipt = check({mode}, 0);
    assert.equal(receipt.validation, 'passed');
    assert.equal(receipt.qualification.correctness, mode === 'contracts' ? 'passed' : 'not_evaluated');
    assert.deepEqual(receipt.binaries.before, receipt.binaries.after);
    assert.deepEqual(receipt.identity_before, receipt.identity_after);
    assert.equal(receipt.results.executed.length, mode === 'contracts' ? 5 : 1);
    assert.equal(receipt.cleanup.roots_empty, true);
    assert.ok(receipt.cleanup.parents.every(row => row.verified));
    if (mode !== 'contracts') assert.equal(receipt.results.measurement.samples, 315);
  });
}

test('correctness verifies both sequential fixtures in the legacy parent', () => {
  const cases = parents.map((name, i) => ({name, output: cleanupOutput(i + 1) +
    (i === 1 ? cleanupOutput(6) : '')}));
  const receipt = check({mode: 'contracts', xml: report(cases)}, 0);
  assert.equal(receipt.results.cleanup.length, 6);
});

for (const fault of ['missing', 'incomplete', 'reused']) {
  test(`legacy parent refuses ${fault} second-fixture cleanup`, () => {
    const cases = parents.map((name, i) => ({name, output: parentOutput(name, i)}));
    cases[1].output = cleanupOutput(2) + (fault === 'missing' ? '' : fault === 'reused' ? cleanupOutput(2) :
      cleanupOutput(6).replace(/DESKTOP_FIXTURE_CHILD=.*"exited".*\n/, ''));
    check({mode: 'contracts', xml: report(cases)}, 1);
  });
}

for (const mode of ['one', 'two']) {
  test(`${mode} diagnostics retain errors without claiming timing acceptance`, () => {
    const output = measurementOutput(mode, true, ({samples}) => { samples[8].outcome = 'frame_deadline'; });
    const receipt = check({mode, diagnostics: true, output}, 0);
    assert.equal(receipt.qualification.measurement, 'diagnostics_only');
    assert.equal(receipt.results.measurement.samples, 45);
    assert.equal(receipt.results.measurement.analysis_samples, 30);
    assert.deepEqual(receipt.results.measurement.outcomes, {frame_deadline: 1});
  });
}

const faults = {
  missing: ({samples}) => samples.pop(),
  duplicate: ({samples}) => { samples[100] = {...samples[99]}; },
  failed: ({samples}) => { samples[100].outcome = 'websocket_refusal_or_transport'; },
  warmup_failure: ({samples}) => { samples[0].outcome = 'frame_reply'; },
  wrong_mode: ({summary}) => { summary.mode = 'two'; },
  wrong_boundary: ({summary}) => { summary.boundary = 'service fixture send'; },
  wrong_schema: ({summary}) => { summary.schema = 2; },
  boolean_schema: ({summary}) => { summary.schema = true; },
  wrong_diagnostics: ({summary}) => { summary.diagnostics = true; },
  diagnostic_sample: ({samples}) => { samples[8].provider = {}; },
  wrong_start: ({starts}) => { starts[12].ordinal = 11; },
  wrong_warmup: ({samples}) => { samples[6].warmup = true; },
  wrong_cold: ({samples}) => { samples[1].cold = true; },
  wrong_ingress: ({samples}) => { samples[1].ingress = 1; },
  negative_time: ({samples}) => { samples[7].milliseconds = -1; },
  nonnumeric_time: ({samples}) => { samples[7].milliseconds = '1'; },
  unknown_outcome: ({samples}) => { samples[7].outcome = 'unknown'; },
};
for (const [name, mutate] of Object.entries(faults)) {
  test(`normal measurement refuses ${name} and retains the original samples`, () => {
    const receipt = check({output: measurementOutput('one', false, mutate)}, 1);
    assert.equal(receipt.collection, 'completed');
    assert.equal(receipt.qualification.measurement, 'rejected');
    assert.ok(receipt.cleanup.parents.every(row => row.verified));
  });
}

test('a 45-sample diagnostic run cannot enter normal measurement acceptance', () => {
  check({output: measurementOutput('one', true)}, 1, /truncated public measurement/);
});
for (const [name, change] of Object.entries({
  truncated_json: output => output.replace(/DESKTOP_PUBLIC_MEASUREMENT=.*/, 'DESKTOP_PUBLIC_MEASUREMENT={'),
  duplicate_summary: output => output + output.split('\n').find(line => line.startsWith('DESKTOP_PUBLIC_MEASUREMENT=')) + '\n',
  duplicate_json_key: output => output.replace('"milliseconds":3.5', '"milliseconds":3.5,"milliseconds":4'),
  missing_cleanup: output => output.replace(/DESKTOP_FIXTURE_CLEANUP=.*\n/, ''),
  child_failure: output => output.replace('"success":true,"code":0', '"success":false,"code":17'),
  missing_child_exit: output => output.replace(/DESKTOP_FIXTURE_CHILD=.*"exited".*\n/, ''),
  live_child: output => output.replaceAll('111111', String(process.pid)),
  changed_existing_inventory: output => output.replace('"after":["' + 'a'.repeat(64) + '"]', '"after":[]'),
  changed_databases: output => output.replace('"owned_absent":true', '"owned_absent":false'),
  aggregate_mismatch: output => output.replace('"schema":1,"mode":"one"', '"schema":1,"samples":[],"mode":"one"'),
})) {
  test(`result validation refuses ${name}`, () => check({output: change(measurementOutput())}, 1));
}

for (const [name, cases] of Object.entries({
  missing: parents.slice(1).map((name, i) => ({name, output: parentOutput(name, i + 1)})),
  duplicate: parents.map((name, i) => ({name: i === 4 ? parents[0] : name, output: parentOutput(name, i)})),
  extra: [...parents, 'unexpected'].map((name, i) => ({name, output: parentOutput(name, i)})),
  failed: parents.map((name, i) => ({name, output: parentOutput(name, i), failure: i === 2 ? '<failure/>' : ''})),
  retried: parents.map((name, i) => ({name, output: parentOutput(name, i), failure: i === 2 ? '<flakyFailure/>' : ''})),
  skipped: parents.map((name, i) => ({name, output: parentOutput(name, i), failure: i === 2 ? '<skipped/>' : ''})),
})) {
  test(`correctness receipts refuse ${name} parents`, () => check({mode: 'contracts', xml: report(cases)}, 1));
}

test('truncated structured output cannot become a completed qualification', () => {
  check({xml: report([{name: measurement, output: measurementOutput()}]).slice(0, -10)}, 1, /Truncated or malformed/);
});
for (const [fault, pattern] of [['binary', /binary identity drift/], ['source', /clean candidate/], ['root', /roots remain/], ['exit', /First attempt failed/]]) {
  test(`runner preserves and refuses ${fault} failure`, () => check({fault}, fault === 'exit' ? 42 : 1, pattern));
}

test('runner pins zero retries and reuses captured binaries rather than compiling at execution', () => {
  const f = fixture({mode: 'contracts'});
  try {
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    const calls = readFileSync(join(f.directory, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    const runs = calls.filter(args => args[0] === 'nextest' && args[1] === 'run');
    assert.equal(runs.length, 1);
    assert.equal(runs[0][runs[0].indexOf('--retries') + 1], '0');
    assert.ok(runs[0].includes('--binaries-metadata'));
    assert.ok(runs[0].includes('--no-tests'));
    const original = f.read('receipt.json');
    assert.notEqual(f.run().status, 0);
    assert.equal(f.read('receipt.json'), original);
  } finally { f.close(); }
});

test('archive reuse requires matching source, toolchain, profile, archive and binary identities', () => {
  const f = fixture({mode: 'contracts'});
  try {
    assert.equal(f.run().status, 0);
    const original = JSON.parse(f.read('artifact-identity.json'));
    const archive = join(f.directory, 'archive.tar.zst');
    writeFileSync(archive, 'synthetic archive bytes');
    original.archive_sha256 = createHash('sha256').update(readFileSync(archive)).digest('hex');
    const metadata = join(f.root, 'api', 'target', 'nextest'); mkdirSync(metadata);
    copyFileSync(join(f.directory, 'cargo-metadata.json'), join(metadata, 'cargo-metadata.json'));
    copyFileSync(join(f.evidence, 'binaries-metadata.json'), join(metadata, 'binaries-metadata.json'));
    for (const field of ['valid', 'source', 'toolchain', 'cargo_profile', 'binaries', 'archive_sha256', 'build_settings_sha256']) {
      const manifest = structuredClone(original);
      if (field !== 'valid') manifest[field] = 'wrong';
      const path = join(f.directory, `${field}.json`); writeFileSync(path, JSON.stringify(manifest));
      const evidence = join(f.directory, `archive-${field}`);
      const run = f.run(['--evidence', evidence, '--archive', archive, '--artifact-manifest', path], {CMA_NEXTEST_REUSE: '1'});
      const receipt = JSON.parse(readFileSync(join(evidence, 'receipt.json'), 'utf8'));
      assert.equal(run.status, field === 'valid' ? 0 : 1, run.stderr + JSON.stringify(receipt));
      if (field !== 'valid') assert.match(receipt.errors.join(' '), /Archive identity mismatch/);
    }
    const validPath = join(f.directory, 'valid.json');
    const driftEvidence = join(f.directory, 'archive-drift');
    const drift = f.run(['--evidence', driftEvidence, '--archive', archive, '--artifact-manifest', validPath],
      {CMA_NEXTEST_REUSE: '1', DESKTOP_TEST_FAULT: 'archive', DESKTOP_TEST_ARCHIVE: archive});
    assert.equal(drift.status, 1);
    assert.match(JSON.parse(readFileSync(join(driftEvidence, 'receipt.json'), 'utf8')).errors.join(' '), /Archive identity drift/);
    const unbound = f.run(['--evidence', join(f.directory, 'unbound')], {CMA_NEXTEST_REUSE: '1'});
    assert.notEqual(unbound.status, 0);
  } finally { f.close(); }
});
