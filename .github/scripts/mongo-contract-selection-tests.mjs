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

// Evaluate the nextest filterset subset the runner uses (test/package atoms,
// not/and/or and parentheses) against one test name. Unsupported syntax fails
// the guard instead of being skipped, so it proves selection semantics.
function filterset(expression) {
  const tokens = [];
  const lexeme = /\s+|(?:and|or|not)\b|[&|()]|(test|package)\(([=~]?)([^()\s]+)\)/y;
  for (let index = 0; index < expression.length; index = lexeme.lastIndex) {
    lexeme.lastIndex = index;
    const match = lexeme.exec(expression);
    assert.ok(match, `unsupported filterset syntax: ${expression.slice(index, index + 60)}`);
    if (match[1]) tokens.push({kind: match[1], exact: match[2] === '=', value: match[3]});
    else if (match[0].trim()) tokens.push(match[0]);
  }
  let position = 0;
  const unary = () => {
    const token = tokens[position++];
    if (token === 'not') { const inner = unary(); return name => !inner(name); }
    if (token === '(') {
      const inner = union();
      assert.equal(tokens[position++], ')', 'balanced filterset');
      return inner;
    }
    assert.equal(typeof token, 'object', 'filterset atom');
    if (token.kind === 'package') {
      assert.ok(token.exact);
      return () => token.value === 'cma';
    }
    return token.exact ? name => name === token.value : name => name.includes(token.value);
  };
  const binary = (operand, operators, combine) => () => {
    let left = operand();
    while (operators.includes(tokens[position])) {
      position++;
      const [l, r] = [left, operand()];
      left = name => combine(l(name), r(name));
    }
    return left;
  };
  const intersection = binary(unary, ['and', '&'], (l, r) => l && r);
  const union = binary(intersection, ['or', '|'], (l, r) => l || r);
  const select = union();
  assert.equal(position, tokens.length, 'complete filterset');
  return select;
}

// Read per-test budgets (period x terminate-after) and fixture output settings.
function nextestBudgets(text) {
  const profiles = {};
  let table = null;
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    let match;
    if (!line || line.startsWith('#')) continue;
    if ((match = /^\[profile\.([a-z-]+)\]$/.exec(line))) {
      table = profiles[match[1]] ??= {overrides: []};
    } else if ((match = /^\[\[profile\.([a-z-]+)\.overrides\]\]$/.exec(line))) {
      table = {};
      (profiles[match[1]] ??= {overrides: []}).overrides.push(table);
    } else if (line.startsWith('[')) {
      table = null;
    } else if (table && (match = /^filter = "([^"]+)"$/.exec(line))) {
      table.filter = match[1];
    } else if (table && (match = /^success-output = "([^"]+)"$/.exec(line))) {
      table.successOutput = match[1];
    } else if (table && line.startsWith('slow-timeout')) {
      match = /^slow-timeout = \{ period = "([1-9][0-9]*)s", terminate-after = ([1-9][0-9]*) \}$/.exec(line);
      assert.ok(match, `unreadable per-test budget: ${line}`);
      table.budget = Number(match[1]) * Number(match[2]);
    }
  }
  return profiles;
}

function captureInvocations(source, modes) {
  const captured = execFileSync('bash', ['-c', selectionFunctions(source) + `
nextest_build() { printf '%s\\n' "$@"; }
for selection in ${modes.join(' ')}; do
  printf 'MODE %s\\n' "$selection"
  nextest_contracts run
done
`], {encoding: 'utf8', env: {...process.env, script_directory: fileURLToPath(new URL('.', import.meta.url))}});
  return Object.fromEntries(captured.split('MODE ').filter(Boolean).map(block => {
    const [mode, ...args] = block.trim().split('\n');
    return [mode, args];
  }));
}
const expression = args => args[args.indexOf('-E') + 1];

export function registerSelectionTests({scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, bsonCeilingTest, firstResponseTest, historyTest, observationTest, fixturePredicate, ordinaryPredicate, qualificationPredicate}) {
  test('selection proof parses old and isolated owner and audit predicates', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith(("readonly owner_allocation_test=", "readonly audit_bounds_test=", "readonly diagnostics_test=", "readonly scan_cooperation_test=", "readonly bson_ceiling_test=", "readonly first_response_test=", "readonly history_test="))
    and not line.strip().startswith(("owner-allocation)", "audit-bounds)", "diagnostics)", "scan-cooperation)", "bson-ceiling)", "first-response)", "history)")))
before = before.replace('not (test(=$scale_test) or test(=$owner_allocation_test) or test(=$audit_bounds_test) or test(=$diagnostics_test) or test(=$scan_cooperation_test) or test(=$bson_ceiling_test) or test(=$first_response_test) or test(=$history_test))', 'not test(=$scale_test)')
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
    ['history', 'history_test', historyTest],
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

  test('selection proof removes the thirty-minute qualification from the listing and ordinary partitions only', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith("readonly observation_budget_test=")
    and not line.strip().startswith("observation-budget)"))
before = before.replace(' or test(=$observation_budget_test)', '')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
      env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
    });
    const {before, after} = JSON.parse(result);
    const {'observation-budget': dedicated, ...remaining} = after;
    assert.equal(before['observation-budget'], undefined);
    assert.ok(dedicated.startsWith(`package(=cma) and test(=${observationTest}) & ${fixturePredicate} & (`));
    for (const mode of ['all', 'ordinary']) {
      assert.equal(remaining[mode].split(` or test(=${observationTest})`).length, 2, `${mode} excludes it exactly once`);
      remaining[mode] = remaining[mode].replace(` or test(=${observationTest})`, '');
    }
    assert.ok(after.all.startsWith(`${qualificationPredicate} & ${fixturePredicate} & (`));
    assert.ok(after.ordinary.startsWith(`${ordinaryPredicate} & ${fixturePredicate} & (`));
    assert.deepEqual(remaining, before);
  });

  // Class guard: every dedicated-mode contract stays out of the PR shards, and
  // only those ordinary partitions run under the per-test budget. Modes come
  // from the runner's own selection cases, so a new mode is guarded too.
  test('ordinary PR partitions exclude every dedicated-mode contract and alone carry the per-test budget', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const modes = [...source.matchAll(/^ {4}([a-z-]+)\) selection_filter=/gm)].map(match => match[1]);
    assert.ok(modes.includes('ordinary') && modes.includes('observation-budget'));
    const invocations = captureInvocations(source, ['all', ...modes]);
    const select = Object.fromEntries(Object.entries(invocations).map(([mode, args]) => [mode, filterset(expression(args))]));
    const named = Object.fromEntries([...source.matchAll(/^readonly ([a-z_]+_test)='([^']+)'$/gm)].map(match => [match[1], match[2]]));
    const declared = Object.keys(JSON.parse(readFileSync(new URL('mongo-contract-exceptions.json', import.meta.url), 'utf8')));
    assert.equal(named.observation_budget_test, observationTest);
    assert.ok(declared.includes(observationTest));
    for (const name of [...Object.values(named), ...declared]) {
      assert.equal(select.ordinary(name), false, `ordinary PR partitions select dedicated contract ${name}`);
    }
    for (const name of declared) assert.equal(select.all(name), false, `the PR listing selects on-demand ${name}`);
    for (const [variable, name] of Object.entries(named)) {
      const owners = modes.filter(mode => expression(invocations[mode]).startsWith(`package(=cma) and test(=${name}) & `));
      assert.equal(owners.length, 1, `${variable} has exactly one dedicated mode`);
      assert.ok(select[owners[0]](name) && source.includes(`"$1" == '--${owners[0]}'`), `--${owners[0]} runs ${variable}`);
    }
    // Exclusions are exact: a sibling contract in the same module stays ordinary.
    const sibling = observationTest.replace(/[^:]+$/, 'observation_two_serving_replicas_keep_idle_work_independent_of_history');
    assert.ok(select.ordinary(sibling) && select.all(sibling));
    const historySibling = historyTest.replace(/[^:]+$/, 'dense_history_batches_preserve_exact_allocation_with_fewer_round_trips');
    assert.ok(select.all(historyTest) && select.history(historyTest) && !select.ordinary(historyTest));
    assert.ok(select.all(historySibling) && select.ordinary(historySibling) && !select.history(historySibling));
    // Not vacuous: without its exclusion the qualification would reach a PR shard.
    const regressed = captureInvocations(source.replaceAll(' or test(=$observation_budget_test)', ''), ['all', 'ordinary']);
    assert.ok(filterset(expression(regressed.ordinary))(observationTest) && filterset(expression(regressed.all))(observationTest));

    for (const [mode, args] of Object.entries(invocations)) {
      assert.equal(args.includes('--profile'), mode === 'ordinary', `${mode} nextest profile`);
    }
    const profile = invocations.ordinary[invocations.ordinary.indexOf('--profile') + 1];
    const budgets = nextestBudgets(readFileSync(new URL('../../api/.config/nextest.toml', import.meta.url), 'utf8'));
    const ordinary = budgets[profile];
    assert.ok(ordinary?.budget > 0, `${profile} terminates a test at its per-test budget`);
    assert.ok(ordinary.budget < 1800, 'the budget stops a thirty-minute qualification');
    assert.equal(budgets.default?.budget, undefined, 'dedicated modes keep the unbounded default profile');
    const timings = JSON.parse(readFileSync(new URL('mongo-contract-timings.json', import.meta.url), 'utf8')).tests;
    const outputOverrides = ordinary.overrides.filter(override => override.successOutput !== undefined);
    assert.deepEqual(outputOverrides, [{filter: `test(=${sibling})`, successOutput: 'final'}]);
    const budgetOverrides = ordinary.overrides.filter(override => override.successOutput === undefined);
    const allowances = Object.fromEntries(budgetOverrides.map(override => {
      const name = /^test\(=([^()\s]+)\)$/.exec(override.filter ?? '')?.[1];
      assert.ok(name && override.budget > ordinary.budget, `override names one exact longer contract: ${override.filter}`);
      assert.ok(select.ordinary(name) && timings[name], `override is for a measured ordinary contract: ${name}`);
      return [name, override.budget];
    }));
    // At least 3x every measured ordinary contract, so the budget cannot flake.
    for (const [name, seconds] of Object.entries(timings)) {
      if (select.ordinary(name)) assert.ok((allowances[name] ?? ordinary.budget) >= 3 * seconds, `${name} (${seconds} s) lacks 3x headroom`);
    }
  });
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
    'scan-cooperation', 'history', 'request-limits', 'agent-list-contracts', 'availability',
    'agent-list-profile', 'organization-inventory', 'agent-lists', 'cpu-profile', 'observation-budget'];
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
