import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import test from 'node:test';

test('desktop fixture selection binds both exact parents and the separately compiled receiver child', () => {
  const result = execFileSync('python3', ['-c', `
import json, runpy, sys
module = runpy.run_path(sys.argv[1])
selected = module['selected']
parents = module['PARENTS']
child = module['CHILD']
def case(): return {'ignored': True, 'filter-match': {'status': 'matches'}}
def suite(package, names):
    return {'package-name': package, 'binary-path': sys.argv[1],
            'testcases': {name: case() for name in names}}
inventory = {'rust-suites': {'cma': suite('cma', parents), 'runtime': suite('layer4-runtime', [child])}}
selected(inventory, 'cma', parents)
selected(inventory, 'layer4-runtime', [child])
print(json.dumps({'parents': parents, 'child': child}))
`, new URL('desktop-replica-contract.py', import.meta.url).pathname], {encoding: 'utf8'});
  const {parents, child} = JSON.parse(result);
  assert.equal(parents.length, 5);
  assert.equal(new Set(parents).size, 5);
  assert.equal(child, 'link::desktop::tests::child_fixture::desktop_receiver_child');
});

test('desktop fixture refuses missing, extra, unignored, duplicate or relocated child inventories', () => {
  const result = execFileSync('python3', ['-c', `
import copy, runpy, sys
module = runpy.run_path(sys.argv[1]); child = module['CHILD']; selected = module['selected']
base = {'rust-suites': {'runtime': {'package-name': 'layer4-runtime', 'binary-path': sys.argv[1],
        'testcases': {child: {'ignored': True, 'filter-match': {'status': 'matches'}}}}}}
for fault in ['missing', 'extra', 'unignored', 'duplicate', 'relative', 'absent', 'unselected']:
    inventory = copy.deepcopy(base); suites = inventory['rust-suites']; runtime = suites['runtime']
    if fault == 'missing': runtime['testcases'].clear()
    if fault == 'extra': runtime['testcases']['unexpected'] = copy.deepcopy(runtime['testcases'][child])
    if fault == 'unignored': runtime['testcases'][child]['ignored'] = False
    if fault == 'duplicate': suites['copy'] = copy.deepcopy(runtime)
    if fault == 'relative': runtime['binary-path'] = 'relative/path'
    if fault == 'absent': runtime['binary-path'] = sys.argv[1] + '/missing'
    if fault == 'unselected': runtime['testcases'][child]['filter-match']['status'] = 'mismatch'
    try: selected(inventory, 'layer4-runtime', [child])
    except ValueError: pass
    else: raise AssertionError(fault)
print('7 refused')
`, new URL('desktop-replica-contract.py', import.meta.url).pathname], {encoding: 'utf8'});
  assert.equal(result.trim(), '7 refused');
});

import {fixture, measurementOutput} from './desktop-replica-fixture.test-helpers.mjs';

test('a collected failed public sample cannot produce a qualification success', () => {
  const output = measurementOutput('one', false, ({samples}) => { samples[7].outcome = 'attempt_deadline'; });
  const f = fixture({output});
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, `failed sample was accepted: ${result.stdout}\n${result.stderr}`);
    assert.match(f.read('contracts.log'), /attempt_deadline/);
    const receipt = JSON.parse(f.read('receipt.json'));
    assert.equal(receipt.first_attempt_exit_code, 0);
    assert.equal(receipt.collection, 'completed');
    assert.match(receipt.errors.join(' '), /non-ok samples/);
  } finally { f.close(); }
});
