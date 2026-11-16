import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export function registerSelectionTests({scaleTest, ownerTest, auditTest, diagnosticsTest, scanTest, fixturePredicate, ordinaryPredicate}) {
  test('selection proof parses old and isolated owner and audit predicates', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith(("readonly owner_allocation_test=", "readonly audit_bounds_test=", "readonly diagnostics_test=", "readonly scan_cooperation_test="))
    and not line.strip().startswith(("owner-allocation)", "audit-bounds)", "diagnostics)", "scan-cooperation)")))
before = before.replace('not (test(=$scale_test) or test(=$owner_allocation_test) or test(=$audit_bounds_test) or test(=$diagnostics_test) or test(=$scan_cooperation_test))', 'not test(=$scale_test)')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
      env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
    });
    const {before, after} = JSON.parse(result);
    assert.equal(before['owner-allocation'], undefined);
    assert.equal(before['audit-bounds'], undefined);
    assert.equal(before.diagnostics, undefined);
    assert.ok(before.ordinary.startsWith(`not test(=${scaleTest}) & ${fixturePredicate} & (`));
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

  test('selection proof preserves the corpus while moving scan cooperation out of ordinary partitions', () => {
    const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
    const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines()
    if not line.startswith("readonly scan_cooperation_test=")
    and not line.strip().startswith("scan-cooperation)"))
before = before.replace(' or test(=$scan_cooperation_test)', '')
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
      env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
    });
    const {before, after} = JSON.parse(result);
    assert.equal(before['scan-cooperation'], undefined);
    assert.ok(after['scan-cooperation'].startsWith(`package(=cma) and test(=${scanTest}) & ${fixturePredicate} & (`));
    assert.equal(after.ordinary.replace(` or test(=${scanTest})`, ''), before.ordinary);
    for (const mode of ['all', 'scale', 'owner-allocation', 'audit-bounds', 'diagnostics', 'cpu-profile']) assert.equal(before[mode], after[mode]);
  });
}


test('focused Agent-list runner keeps the complete ordinary and full selections', () => {
  const source = readFileSync(new URL('mongo-contract-tests.sh', import.meta.url), 'utf8');
  const result = execFileSync('python3', ['-c', `
import json, runpy, sys
parse = runpy.run_path(sys.argv[1])["mongo_filters"]
source = sys.argv[2]
before = "\\n".join(line for line in source.splitlines() if not line.strip().startswith("agent-lists)"))
print(json.dumps({"before": parse(before), "after": parse(source)}))
`, fileURLToPath(new URL('backend-selection-proof.py', import.meta.url)), source], {
    env: {...process.env, PYTHONDONTWRITEBYTECODE: '1'}, encoding: 'utf8',
  });
  const {before, after} = JSON.parse(result);
  assert.deepEqual(after, before);
  assert.ok(source.includes("agent-lists) selection_filter='package(=cma) and test(::resource_access_tests::personal_inventory::)'"));
  assert.ok(source.includes(`"$1" == '--agent-lists'`));
});
