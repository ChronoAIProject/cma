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
  const features = writerCoverage(actual, [], undefined, 'features');
  const prepared = writerCoverage(actual, [], undefined, 'prepared');
  const groups = [...features.groups, ...prepared.groups];
  assert.equal(features.total + prepared.total, 13);
  assert.equal(new Set(groups.map(row => row.id)).size, 13);
  assert.equal(features.groups.at(-1).id, 'SW12');
  assert.deepEqual(prepared.groups.map(row => row.id), ['SW05', 'SW06', 'SW13']);
  assert.equal(groups.find(row => row.id === 'SW09').verdict, 'NOT_RUN');
  assert.ok(groups.filter(row => row.id !== 'SW09').every(row => row.verdict === 'BLOCKED'));
  for (const fixture of ['features', 'prepared']) {
    for (const malformed of [actual.cases.slice(0, 12), [...actual.cases.slice(0, 12), actual.cases[0]]]) {
      assert.throws(() => writerCoverage({...actual, cases: malformed}, [], undefined, fixture), /writer_registry_ids/);
    }
  }
  assert.throws(() => writerCoverage(actual, [], undefined, 'invalid'), /writer_fixture/);
});

test('complete mapping needs all distinct repetitions and both restart phases', () => {
  const registry = complete(), rows = observations();
  const evaluate = selected => writerCoverage(registry, selected, undefined, 'features');
  assert.equal(evaluate(rows).verdict, 'PASS');
  assert.equal(evaluate(rows.filter(row => row.phase === 'before_restart')).verdict, 'FAIL');
  assert.equal(evaluate(rows.filter(row => row.case !== 'case_11')).groups.at(-1).verdict, 'NOT_RUN');
  rows[0].non_owner = false;
  assert.equal(evaluate(rows).groups[0].verdict, 'FAIL');
});

test('partial assertions and journey-only evidence never complete missing barriers', () => {
  const registry = complete(), rows = observations();
  const evaluate = journey => writerCoverage(registry, rows, journey, 'features');
  registry.cases[0].status = 'INCOMPLETE_NOT_EXECUTION_PASS';
  registry.cases[0].remaining = 'native publication barrier';
  assert.equal(evaluate().groups[0].verdict, 'BLOCKED');
  registry.cases[1].required_assertions = [];
  assert.equal(evaluate().groups[1].verdict, 'BLOCKED');
  registry.cases[2].required_assertions = [{case: 'restart', routing: 'journey', minimum_runs: 1, phases: []}];
  const journey = {steps: [{name: 'restart', verdict: 'PASS'}], verdict: 'FAIL'};
  assert.equal(evaluate(journey).groups[2].verdict, 'FAIL');
  journey.verdict = 'LOCAL_TWO_AM_JOURNEY_PASS';
  assert.equal(evaluate(journey).groups[2].verdict, 'PASS');
  assert.equal(evaluate(journey).verdict, 'FAIL');
});

test('prepared coverage requires the scoped abort, both ingress steps and a completed journey', async () => {
  const registry = JSON.parse(await readFile(new URL('../writer-regressions.json', import.meta.url)));
  const group = registry.cases.find(row => row.id === 'SW13');
  const abort = registry.cases.find(row => row.id === 'SW05');
  const refusal = registry.cases.find(row => row.id === 'SW06');
  const journey = {verdict: 'LOCAL_PREPARED_TWO_AM_PASS',
    steps: [...group.required_assertions, ...refusal.required_assertions].map(row => ({name: row.case, verdict: 'PASS'}))};
  const evaluate = () => writerCoverage(registry, [], journey, 'prepared');
  assert.equal(evaluate().groups[2].verdict, 'BLOCKED');
  group.status = 'COMPLETE';
  assert.equal(evaluate().verdict, 'FAIL');
  abort.status = 'COMPLETE';
  assert.equal(evaluate().verdict, 'FAIL');
  refusal.status = 'COMPLETE';
  assert.equal(evaluate().verdict, 'PASS');
  journey.verdict = 'LOCAL_TWO_AM_JOURNEY_PASS';
  assert.equal(evaluate().verdict, 'FAIL');
  journey.verdict = 'LOCAL_PREPARED_TWO_AM_PASS';
  journey.steps.pop();
  assert.deepEqual(evaluate().groups.map(row => row.verdict), ['PASS', 'NOT_RUN', 'PASS']);
  journey.steps.pop();
  assert.deepEqual(evaluate().groups.map(row => row.verdict), ['NOT_RUN', 'NOT_RUN', 'NOT_RUN']);
});

test('lifecycle Always on coverage requires browser execution on both sides of restart', async () => {
  const registry = JSON.parse(await readFile(new URL('../writer-regressions.json', import.meta.url)));
  const caseName = 'writer.lifecycle_owner_relinquishment_browser';
  const evaluate = rows => writerCoverage(registry, rows, undefined, 'features').groups
    .find(group => group.id === 'SW09').assertions.find(assertion => assertion.case === caseName).verdict;
  const rows = ['before_restart', 'after_restart'].map(phase => ({case: caseName, phase, iteration: 1,
    ams: ['a', 'b'], real_browser: true, verdict: 'PASS'}));
  assert.equal(evaluate(rows), 'PASS');
  assert.equal(evaluate(rows.slice(0, 1)), 'FAIL');
  assert.equal(evaluate([rows[0], {...rows[1], real_browser: false}]), 'FAIL');
});
