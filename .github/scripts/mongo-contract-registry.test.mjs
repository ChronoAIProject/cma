import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const registry = fileURLToPath(new URL('mongo-contract-registry.py', import.meta.url));
function fixture(t, entries = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'mongo-suite-registry-'));
  t.after(() => rmSync(directory, {recursive: true, force: true}));
  for (const [name, value] of Object.entries(entries)) {
    writeFileSync(join(directory, name), JSON.stringify(value));
  }
  return directory;
}
function load(directory) {
  return spawnSync('python3', ['-B', '-c', `
import json, pathlib, runpy, sys
registry = runpy.run_path(sys.argv[1])
directory = pathlib.Path(sys.argv[2])
print(json.dumps({"prefixes":registry["load_prefixes"](directory),
                  "filter":registry["suite_filter"](directory)}))
`, registry, directory], {encoding: 'utf8'});
}

test('independent suite files extend the selector without changing existing entries', t => {
  const directory = fixture(t, {'z-suite.json': {prefix: 'z_suite::mongo_tests::'}});
  const original = load(directory);
  assert.equal(original.status, 0, original.stderr);
  assert.deepEqual(JSON.parse(original.stdout).prefixes, ['z_suite::mongo_tests::']);
  writeFileSync(join(directory, 'a-suite.json'), JSON.stringify({prefix: 'a_suite::tests::'}));
  const extended = load(directory);
  assert.equal(extended.status, 0, extended.stderr);
  assert.deepEqual(JSON.parse(extended.stdout), {
    prefixes: ['a_suite::tests::', 'z_suite::mongo_tests::'],
    filter: 'test(a_suite::tests::) | test(z_suite::mongo_tests::)',
  });
});

test('overlapping parent and child registrations retain the existing prefix semantics', t => {
  const directory = fixture(t, {
    'parent.json': {prefix: 'suite::'},
    'child.json': {prefix: 'suite::nested::'},
  });
  const result = load(directory);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout).prefixes, ['suite::', 'suite::nested::']);
});

test('duplicate prefixes name both files instead of silently discarding a registration', t => {
  const directory = fixture(t, {
    'first.json': {prefix: 'suite::tests::'},
    'second.json': {prefix: 'suite::tests::'},
  });
  const result = load(directory);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Duplicate Mongo suite prefix in first.json and second.json/);
});

test('malformed registrations cannot inject a selector or silently lose coverage', t => {
  const directory = fixture(t);
  for (const value of [null, [], {}, {prefixes: ['suite::']}, {prefix: 'suite::', extra: true},
    {prefix: null}, {prefix: 3}, {prefix: ''}, {prefix: 'suite'}, {prefix: 'suite::\n'},
    {prefix: 'suite::) or all()'}, {prefix: '~suite::'}, {prefix: 'suite::*'}]) {
    writeFileSync(join(directory, 'invalid.json'), JSON.stringify(value));
    const result = load(directory);
    assert.notEqual(result.status, 0, JSON.stringify(value));
    assert.match(result.stderr, /invalid.json/);
  }
  writeFileSync(join(directory, 'invalid.json'), '{');
  const malformed = load(directory);
  assert.notEqual(malformed.status, 0);
  assert.match(malformed.stderr, /Invalid JSON in invalid.json/);
});

test('missing, empty and non-file registries fail closed', t => {
  const directory = fixture(t);
  assert.match(load(directory).stderr, /must not be empty/);
  assert.notEqual(load(join(directory, 'missing')).status, 0);
  mkdirSync(join(directory, 'nested.json'));
  assert.match(load(directory).stderr, /Expected a regular suite JSON file/);
  rmSync(join(directory, 'nested.json'), {recursive: true});
  writeFileSync(join(directory, 'typo.txt'), '{}');
  assert.match(load(directory).stderr, /Expected a regular suite JSON file/);
  rmSync(join(directory, 'typo.txt'));
  symlinkSync(registry, join(directory, 'linked.json'));
  assert.match(load(directory).stderr, /Expected a regular suite JSON file/);
});

test('selection proof reads the requested baseline registry and still supports inline predecessors', t => {
  const directory = fixture(t, {'baseline.json': {prefix: 'baseline_only::tests::'}});
  const result = spawnSync('python3', ['-B', '-c', `
import json, pathlib, runpy, sys
proof = pathlib.Path(sys.argv[1])
parse = runpy.run_path(str(proof))["mongo_filters"]
source = proof.with_name("mongo-contract-tests.sh").read_text()
baseline = pathlib.Path(sys.argv[2])
print(json.dumps({"registered":parse(source, baseline),
                  "inline":parse(source.replace("$suite_filter", "test(old_inline::)"), baseline / "missing")}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), directory], {encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr);
  const {registered, inline} = JSON.parse(result.stdout);
  assert.deepEqual(Object.keys(registered), Object.keys(inline));
  for (const mode of Object.keys(registered)) {
    assert.ok(registered[mode].endsWith(' & (test(baseline_only::tests::))'));
    assert.equal(inline[mode], registered[mode].replace('test(baseline_only::tests::)', 'test(old_inline::)'));
  }
});
