import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { requiredPrefixes } from './mongo-contract-test-prefixes.mjs';

const taskHome = 'sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::';
const child = `${taskHome}subprocess::task_home_create_child`;
const caseMetadata = (selected, ignored = true) => ({
  ignored, 'filter-match': {status: selected ? 'matches' : 'mismatch'},
});
const exceptions = {[child]: {kind: 'helper', reason: 'Invoked by its process-exit parent.'}};
function inventory() {
  return {'rust-suites': {'cma::bin/cma': {
    'package-name': 'cma', 'binary-name': 'cma', kind: 'bin',
    testcases: {
      [`${taskHome}subprocess::initial_task_public_service_process_exit_preserves_admission`]: caseMetadata(true),
      'a_new_contract_module::new_mongo_contract': caseMetadata(true),
      [child]: caseMetadata(false),
      'ordinary_unit_test': caseMetadata(false, false),
    },
  }}};
}
function check(metadata, exclusions = exceptions) {
  return spawnSync('python3', ['-c', `
import json, runpy, sys
check = runpy.run_path(sys.argv[1])["check_selection"]
metadata, exceptions = json.load(sys.stdin)
try:
    check(metadata, exceptions)
except (ValueError, KeyError, TypeError) as error:
    raise SystemExit(str(error))
`, new URL('mongo-contract-completeness.py', import.meta.url).pathname], {
    input: JSON.stringify([metadata, exclusions]), encoding: 'utf8',
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'},
  });
}

test('metadata completeness accepts selected contracts and reports exact helper exclusions', () => {
  const result = check(inventory());
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /3 ignored CMA tests; 2 selected; 0 separate-fixture tests; 1 child helpers; 0 tracked deferrals; 0 unclassified omissions/);
});

test('metadata completeness catches omitted task-home and newly introduced modules independently of the prefix inventory', () => {
  for (const name of [
    `${taskHome}subprocess::initial_task_public_service_process_exit_preserves_admission`,
    'a_new_contract_module::new_mongo_contract',
    `${child}_another_contract`,
  ]) {
    const metadata = inventory();
    metadata['rust-suites']['cma::bin/cma'].testcases[name] = caseMetadata(false);
    const result = check(metadata);
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes(`Omitted eligible Mongo contracts:\n  ${name}`), result.stderr);
  }
});

test('metadata completeness rejects helper selection and stale or undocumented exceptions', () => {
  const metadata = inventory();
  metadata['rust-suites']['cma::bin/cma'].testcases[child] = caseMetadata(true);
  assert.match(check(metadata).stderr, /Selected non-ordinary contracts/);
  assert.match(check(inventory(), {...exceptions, removed: exceptions[child]}).stderr, /Stale exceptions/);
  for (const exception of [{kind: 'helper'}, {kind: 'unknown', reason: 'unknown'}, {kind: 'deferred', reason: 'broken'}]) {
    assert.equal(check(inventory(), {[child]: exception}).status, 1);
  }
});

test('metadata completeness refuses missing binary inventory, malformed test metadata and nonignored selections', () => {
  for (const mutate of [
    metadata => { metadata['rust-suites'] = {}; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].kind = 'lib'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases = {}; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases[child].ignored = 'true'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases[child]['filter-match'].status = 'unknown'; },
    metadata => { metadata['rust-suites']['cma::bin/cma'].testcases.ordinary_unit_test = caseMetadata(true, false); },
  ]) {
    const metadata = inventory();
    mutate(metadata);
    assert.equal(check(metadata).status, 1);
  }
});

test('tracked deferrals stay visible instead of being reported as eligible coverage', () => {
  const result = check(inventory(), {[child]: {
    kind: 'deferred', reason: 'Separate tracked failure.',
    issue: 'https://github.com/ChronoAIProject/cma/issues/1854',
  }});
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /1 tracked deferrals/);
  assert.match(result.stdout, /Deferred Mongo contract: .*issues\/1854/);
});

test('prefix inventory includes task-home and agrees with the runner without duplicates', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const line = source.split('\n').find(line => line.trimStart().startsWith('-E '));
  const actual = [...line.matchAll(/(?:'|\| )test\(([^)]+)\)/g)].map(match => match[1]);
  assert.deepEqual(actual, requiredPrefixes);
  assert.equal(new Set(requiredPrefixes).size, 129);
  assert.ok(requiredPrefixes.includes(taskHome));
  assert.ok(line.includes(`not test(=${child})`));
});
