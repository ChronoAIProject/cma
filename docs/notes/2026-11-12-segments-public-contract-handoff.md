## Integration allocation: capacity route contracts, 2026-11-12

The integration owner completes the missing public contract registration under
the existing instruction to fix integration-only breakage. Recovery behavior,
authorization and default-closed activation are unchanged. Exact paths:

- `api/crates/layer1-public/src/rest/agent_capacity_recovery.rs` and
  `agent_capacity_recovery/commands.rs`: documentation annotations/exports only.
- `api/crates/layer1-public/src/rest/openapi/composition.rs`: document merge.
- `api/crates/layer1-public/src/rest/public_contract_tests.rs` and new
  `public_contract_tests/capacity_recovery_cases.rs` plus its fixture child.
- `public_contract_tests/inventory.rs`, `tenant_isolation/inventory.rs` only
  if explicit contract wiring requires it; no exclusion or whitelist change.
- `api/crates/layer1-public/tests/fixtures/public-contract-inventory.json`,
  `tenant-isolation-matrix.json` and new capacity recovery corpus files.
- `docs/API_REST.md`: mounted route contract documentation.

The pending slice-owner handoff will be merged normally if it arrives; duplicate
metadata/test changes will be reconciled without weakening either contract.

The shared corpus requires typed SDK dispatch for every documented route.
Additional integration-only ownership: `sdk/rust/src/agents/capacity_recovery.rs`,
its `agents/mod.rs` registration, `sdk/rust/tests/public_contract.rs` and new
`public_contract/capacity_recovery.rs`; `sdk/typescript/src/agents/capacity-recovery.ts`,
its `agents.ts`/`index.ts` exports, `sdk/typescript/tests/public-contract.test.ts`
and new `public-contract/capacity-recovery.ts`. These wrappers preserve original
requests and expose no automatic admission, retry or activation.
