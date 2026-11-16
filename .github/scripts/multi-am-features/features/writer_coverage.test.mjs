import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {writerCoverage} from '../writer_coverage.mjs';

const complete = () => ({schema: 'multi-am-writer-regressions-v2', cases:
  Array.from({length: 13}, (_, i) => ({id: 'SW' + String(i + 1).padStart(2, '0'),
    status: 'COMPLETE', required_assertions: [{case: 'case_' + i, routing: 'non_owner',
      minimum_runs: 3, phases: ['before_restart', 'after_restart']}]}))});
const observations = () => complete().cases.flatMap(group => ['before_restart', 'after_restart']
  .flatMap(phase => [1, 2, 3].map(iteration => ({case: group.required_assertions[0].case,
    phase, iteration, non_owner: true, verdict: 'PASS'}))));

test('all thirteen writer groups remain required independently of C275', async () => {
  const actual = JSON.parse(await readFile(new URL('../writer-regressions.json', import.meta.url)));
  const result = writerCoverage(actual, []);
  assert.equal(result.total, 13);
  assert.equal(result.verdict, 'FAIL');
  assert.equal(result.groups.at(-1).id, 'SW13');
  assert.ok(result.groups.every(row => row.verdict === 'BLOCKED'));
  for (const malformed of [actual.cases.slice(0, 12), [...actual.cases.slice(0, 12), actual.cases[0]]]) {
    assert.throws(() => writerCoverage({...actual, cases: malformed}, []), /writer_registry_ids/);
  }
});

test('complete mapping needs all distinct repetitions and both restart phases', () => {
  const registry = complete(), rows = observations();
  assert.equal(writerCoverage(registry, rows).verdict, 'PASS');
  assert.equal(writerCoverage(registry, rows.filter(row => row.phase === 'before_restart')).verdict, 'FAIL');
  assert.equal(writerCoverage(registry, rows.filter(row => row.case !== 'case_12')).groups[12].verdict, 'NOT_RUN');
  rows[0].non_owner = false;
  assert.equal(writerCoverage(registry, rows).groups[0].verdict, 'FAIL');
});

test('partial assertions and journey-only evidence never complete missing barriers', () => {
  const registry = complete(), rows = observations();
  registry.cases[0].status = 'INCOMPLETE_NOT_EXECUTION_PASS';
  registry.cases[0].remaining = 'native publication barrier';
  assert.equal(writerCoverage(registry, rows).groups[0].verdict, 'BLOCKED');
  registry.cases[1].required_assertions = [];
  assert.equal(writerCoverage(registry, rows).groups[1].verdict, 'BLOCKED');
  registry.cases[2].required_assertions = [{case: 'restart', routing: 'journey', minimum_runs: 1, phases: []}];
  const journey = {steps: [{name: 'restart', verdict: 'PASS'}], verdict: 'FAIL'};
  assert.equal(writerCoverage(registry, rows, journey).groups[2].verdict, 'FAIL');
  journey.verdict = 'LOCAL_TWO_AM_JOURNEY_PASS';
  assert.equal(writerCoverage(registry, rows, journey).groups[2].verdict, 'PASS');
  assert.equal(writerCoverage(registry, rows, journey).verdict, 'FAIL');
});
