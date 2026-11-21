import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

function selectionFunctions(source) {
  const start = source.indexOf('readonly scale_test=');
  const end = source.indexOf('\nif [[ $# == 0 ');
  assert.ok(start >= 0 && end > start, 'runner selection function boundaries exist');
  return source.slice(start, end);
}

export function registerSelectionTests({scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, fixturePredicate, ordinaryPredicate, qualificationPredicate}) {
  test('selection proof parses old and isolated owner and audit predicates', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith(("readonly owner_allocation_test=", "readonly audit_bounds_test=", "readonly diagnostics_test=", "readonly scan_cooperation_test=", "readonly bson_ceiling_test=", "readonly first_response_test="))
    and not line.strip().startswith(("owner-allocation)", "audit-bounds)", "diagnostics)", "scan-cooperation)", "bson-ceiling)", "first-response)")))
before = before.replace('not (test(=$scale_test) or test(=$owner_allocation_test) or test(=$audit_bounds_test) or test(=$diagnostics_test) or test(=$scan_cooperation_test) or test(=$bson_ceiling_test) or test(=$first_response_test))', 'not test(=$scale_test)')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
      env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
    });
    const {before, after} = JSON.parse(result);
    assert.equal(before['owner-allocation'], undefined);
    assert.equal(before['audit-bounds'], undefined);
    assert.equal(before.diagnostics, undefined);
    assert.ok(before.ordinary.startsWith(`not test(=${scaleTest}) & ${qualificationPredicate} & ${fixturePredicate} & (`));
    assert.ok(after.ordinary.startsWith(`${ordinaryPredicate} & ${fixturePredicate} & (`));
    assert.ok(after['owner-allocation'].startsWith(`package(=cma) and test(=${ownerTest}) & ${fixturePredicate} & (`));
    assert.ok(after['audit-bounds'].startsWith(`package(=cma) and test(=${auditTest}) & ${fixturePredicate} & (`));
    for (const mode of ['all', 'scale', 'cpu-profile']) assert.equal(before[mode], after[mode]);
  });

  test('selection proof preserves the corpus while moving diagnostics out of ordinary partitions', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith("readonly diagnostics_test=")
    and not line.strip().startswith("diagnostics)"))
before = before.replace(' or test(=$diagnostics_test)', '')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
      env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
    });
    const {before, after} = JSON.parse(result);
    assert.equal(before.diagnostics, undefined);
    assert.ok(after.diagnostics.startsWith(`package(=cma) and test(=${diagnosticsTest}) & ${fixturePredicate} & (`));
    assert.equal(after.ordinary.replace(` or test(=${diagnosticsTest})`, ''), before.ordinary);
    for (const mode of ['all', 'scale', 'owner-allocation', 'audit-bounds', 'cpu-profile']) assert.equal(before[mode], after[mode]);
  });

  for (const [mode, variable, testName] of [
    ['scan-cooperation', 'scan_cooperation_test', scanTest],
    ['bson-ceiling', 'bson_ceiling_test', bsonCeilingTest],
    ['first-response', 'first_response_test', firstResponseTest],
  ]) {
    test(`selection proof moves only ${mode} from ordinary partitions`, () => {
      const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
      const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source, mode, variable = sys.argv[2:]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith(f"readonly {variable}=")
    and not line.strip().startswith(f"{mode})"))
before = before.replace(f' or test(=$\{variable})', '')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source, mode, variable], {
        env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
      });
      const {before, after} = JSON.parse(result);
      const {[mode]: isolated, ...remaining} = after;
      assert.ok(isolated.startsWith(`package(=cma) and test(=${testName}) & ${fixturePredicate} & (`));
      remaining.ordinary = remaining.ordinary.replace(` or test(=${testName})`, '');
      assert.deepEqual(remaining, before);
    });
  }
}

test('focused Agent-list runner keeps the complete ordinary and full selections', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines() if not (line.strip().startswith("agent-lists)") or line.strip().startswith("agent-list-profile)")))
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
  });
  const {before, after} = JSON.parse(result);
  assert.deepEqual(after, before);
  assert.ok(source.includes("agent-lists) selection_filter='package(=cma) and not test(::personal_inventory::availability::) and not (test(::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile)) and (test(::resource_access_tests::personal_inventory::) or test(::resource_access_tests::inventory::) or test(::resource_access_tests::provenance::) or test(::agent_task_status::batch::tests::) or test(::agent_task_status::activity::tests::) or test(::agent_admission::mongo_integration_tests::channel_usage::) or test(::agent_admission::mongo_integration_tests::provenance_inventory::))'"));
  assert.ok(source.includes(`"$1" == '--agent-lists'`));
  assert.ok(source.includes("agent-list-profile) selection_filter='package(=cma) and (test(::resource_access_tests::personal_inventory::concurrency::personal_agent_list_phase_profile) or test(::resource_access_tests::personal_inventory::concurrency::personal_agent_provenance_phase_profile))'"));
  assert.ok(source.includes(`"$1" == '--agent-list-profile'`));
  assert.ok(source.includes("organization-inventory) selection_filter='package(=cma) and (test(::resource_access_tests::inventory::) or test(::resource_access_tests::personal_inventory::organization::))'"));
  assert.ok(source.includes(`"$1" == '--organization-inventory'`));
});

// Execute the selection function with a capturing nextest port. This proves
// ordinary/list modes and explicit diagnostics, rather than matching one line.
test('ordinary CI excludes exact list load diagnostics but retains correctness contracts', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const prefix = selectionFunctions(source);
  const captured = execFileSync('bash', ['-c', prefix + `
nextest_build() { printf '%s\\n' "$@"; }
for selection in all ordinary agent-lists agent-list-contracts agent-list-profile availability organization-inventory; do
  printf 'MODE %s\\n' "$selection"
  nextest_contracts list
 done
`], {encoding: 'utf8', env: {...process.env, script_directory: fileURLToPath(new URL('.', import.meta.url))}});
  const modes = Object.fromEntries(captured.split('MODE ').filter(Boolean).map(block => {
    const [name, ...args] = block.trim().split('\n');
    return [name, args[args.indexOf('-E') + 1]];
  }));
  const exceptions = JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8'));
  const diagnostics = Object.keys(exceptions).filter(name => name.includes('::personal_inventory::concurrency::') || name.includes('::personal_inventory::availability::') || name === 'store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::organization::organization_one_hundred_concurrent_public_readers_meet_budgets');
  assert.equal(diagnostics.length, 6);
  for (const mode of ['all', 'ordinary']) {
    const excluded = modes[mode].match(/not \(test\(=store::sandbox_mongo_integration_tests::resource_access_tests::personal_inventory::concurrency::[^\n]*?\)\)/)?.[0];
    assert.ok(excluded, `${mode} has an explicit qualification exclusion`);
    for (const name of diagnostics) assert.ok(excluded.includes(`test(=${name})`), `${mode} excludes ${name}`);
    assert.ok(modes[mode].includes('test(store::sandbox_mongo_integration_tests::resource_access_tests::)'));
    assert.ok(!modes[mode].includes('not test(::resource_access_tests::personal_inventory::)'));
  }
  assert.ok(modes['agent-list-contracts'].startsWith('package(=cma) and test(::resource_access_tests::personal_inventory::contracts::)'));
  for (const mode of ['agent-lists', 'agent-list-profile', 'availability', 'organization-inventory']) {
    assert.ok(!modes[mode].includes(`test(=${diagnostics[0]})`), `${mode} must explicitly select its own workload`);
  }
});

test('request-limit selection preserves all existing runner selections', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines() if not line.strip().startswith("request-limits)"))
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
  });
  const {before, after} = JSON.parse(result);
  assert.deepEqual(after, before);
  assert.ok(source.includes("request-limits) selection_filter='package(=cma) and (test(store::request_limits::tests::) or test(rate_limit::lease::mongo_tests::) or test(rate_limit::anonymous_mongo_tests::) or test(rate_limit::control_tests::mongo::) or test(rate_limit::input_tests::mongo::))'"));
  assert.ok(source.includes(`"$1" == '--request-limits'`));
});

test('merged runner selections share the complete contract corpus and fixture exclusions', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const modes = ['all', 'ordinary', 'scale', 'owner-allocation', 'audit-bounds', 'diagnostics',
    'scan-cooperation', 'request-limits', 'agent-list-contracts', 'availability',
    'agent-list-profile', 'organization-inventory', 'agent-lists', 'cpu-profile'];
  const prefix = selectionFunctions(source);
  const captured = execFileSync('bash', ['-c', prefix + `
nextest_build() { printf '%s\\n' "$@"; }
for selection in ${modes.join(' ')}; do
  printf 'MODE %s\\n' "$selection"
  nextest_contracts list
done
`], {encoding: 'utf8', env: {...process.env, script_directory: fileURLToPath(new URL('.', import.meta.url))}});
  const filters = Object.fromEntries(captured.split('MODE ').filter(Boolean).map(block => {
    const [name, ...args] = block.trim().split('\n');
    return [name, args[args.indexOf('-E') + 1]];
  }));
  const boundary = ' & not test(=auth::account_standing::mongo_tests::replicas::fresh_snapshot_cannot_mask_real_database_permission_failure)';
  const corpus = filters.all.slice(filters.all.indexOf(boundary));
  assert.ok(filters.all.includes(boundary));
  for (const mode of modes) {
    assert.equal(filters[mode].slice(filters[mode].indexOf(boundary)), corpus, mode);
    if (mode !== 'all') assert.notEqual(filters[mode], filters.all, `${mode} must select its own workload`);
  }
  assert.equal(filters['request-limits'], 'package(=cma) and (test(store::request_limits::tests::) or test(rate_limit::lease::mongo_tests::) or test(rate_limit::anonymous_mongo_tests::) or test(rate_limit::control_tests::mongo::) or test(rate_limit::input_tests::mongo::))' + corpus);
});
