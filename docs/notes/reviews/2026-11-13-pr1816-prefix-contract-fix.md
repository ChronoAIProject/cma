# #1816 Mongo runner inventory correction

Parent candidate: `c7dc388fc856fad71d501eca93154f78cb6f6953`.
CI run `43361393726`, [job `131675491095`](https://github.com/ChronoAIProject/cma/actions/runs/43361393726/job/131675491095),
failed four staging regressions. The source and archive variants of
`mongo-contract-tests.test.mjs:194` and `:220` compare complete Cargo argument
lists with `assert.deepEqual`. Their actual `-E` expression included three
operator-admission prefixes absent from the expected `requiredPrefixes` list.
Removing exactly those additions from the logged actual arguments produces the
expected arguments, identifying a deterministic inventory mismatch.
Before-job log SHA-256: `2ef323b168c06e8cee3f88d9861b707a151b10c96452c1f9f2db60ae131c3c93`.

The same assertions passed on #1837 run `43358668164`, #1839 run `43360982474`,
and #1846's merged-develop run `43357581319`. The last run's staging step passed
before the overall run was cancelled. This failure was introduced by #1816's
runner-filter extension; it was not an infrastructure failure.

The correction adds exactly the runner's three ordered prefixes to
`.github/scripts/mongo-contract-test-prefixes.mjs`. The runner and all test
assertions remain byte-identical to the parent candidate. The corrected
inventory's 82 entries exactly match the runner filter in order.
Corrected inventory SHA-256:
`ed9d98afbffcf680eb49e2c0e0cfe6d5350fed5bf4d39734a24eac45ba5508d1`.

## Validation

The exact CI staging command ran under Node v26.13.1:

```text
node --test .github/scripts/staging-authority.test.mjs .github/scripts/deployment-observer.test.mjs .github/scripts/mongo-contract-tests.test.mjs
```

It exited zero: **90 passed, zero failed, one skipped**, including all four
previously failing assertions. The skip is the existing optional real-kubectl
loopback projection fixture. No assertion or timeout was changed.
After-log SHA-256: `b4fe45182fce95c6e543914aaedc0c04d97932bf14f7ca5760f8b9c431d76b06`.

The real `mongo-contract-tests.sh --list` ran under build-slot with
`RUST_MIN_STACK=33554432` and exited zero. Its complete selected inventory is
unchanged at **1,141 tests**. Each required operator prefix remains selected:

| Prefix | Selected tests |
| --- | --- |
| `store::admission_control::mongo_tests::` | 11 |
| `store::agent_lifecycle_operations::operator_stop_tests::` | 7 |
| `store::sandbox_mongo_integration_tests::command_ledger_tests::operator_stop_tests::` | 5 |

Discovery SHA-256: `300aae69a9d8e9ad92807184377c881586a4377415c0622d4e1f7b764222efce`;
this is byte-identical to the final discovery recorded before the correction.
Production source, writer defaults and the approved PR-A scope are unchanged.
Independent review must extend to the correction's exact published head before
its separately authorized merge slot. The merge body remains exactly
`Closes #1849`; #1478 remains open.
