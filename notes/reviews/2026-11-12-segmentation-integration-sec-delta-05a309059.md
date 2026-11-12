PR #1766 security blocker re-check at 05a309059f52117ac3ac236098640ba1b2706422

Scope: exact delta from f3b022ba6d7142004d644317b52484f805765d52; closure of B1/B2 from `notes/reviews/2026-11-12-segmentation-integration-sec-delta.md` and new authorization/tenant-isolation exposure. The PR head was independently verified against GitHub. This is a read-only delta review, not production activation approval.

**B1 closed.** `api/crates/layer1-public/tests/fixtures/public-contract/capacity-recovery-dry-run.json:22` now expects observed revision 1 while retaining log high-water 9. This matches the existing seed and production preflight semantics. The fixture change is one numeric value; backend full-response equality, authorization setup, production preflight and the other three dry-run refusal cases are unchanged. Backend replay result is recorded below.

**B2 closed.** `sdk/rust/src/agents/capacity_recovery.rs:140` and `sdk/typescript/src/agents/capacity-recovery.ts:125` recognize only HTTP 409 with the exact single-field known-abort body as a typed outcome. Other 409 bodies still use the standard problem decoder, preserving API-error code, correlation and retry metadata. Unknown remains HTTP 202, recorded remains HTTP 200, and status/outcome mismatches refuse. Neither method retries or generates a different original request.

The contract-runner exception is narrowly limited to the two recovery command operations, status 409 and that exact known-abort body. It routes the case through typed result equality instead of skipping it: `sdk/rust/tests/public_contract.rs:267` with `public_contract/context.rs:320`; `sdk/typescript/tests/public-contract.test.ts:256` with the equality assertion at `:279`. Dedicated tests add positive known-abort decoding, ordinary conflict metadata, malformed bodies, extra fields, wrong statuses, exact original body and exactly one request (`sdk/rust/tests/capacity_recovery.rs:46`; `sdk/typescript/tests/capacity-recovery.test.ts:13`). No assertion was replaced by a blanket success allowance.

**No new authorization or tenant-isolation exposure found.** The 15-file delta contains no change to production backend handlers, route composition, resource authorization, recovery service/state/runtime, writer/publication or transaction code. All five recovery routes retain the previous authenticated, exact-Agent authorization and concealed foreign/missing behavior. Existing S4/S5 security conclusions therefore remain applicable to those unchanged production files.

The new TypeScript internal transport helper (`sdk/typescript/src/http/outcome.ts:5`) keeps the existing credential/header construction, URL handling, request deadline and bounded JSON read. Its status allowance is supplied explicitly by the recovery command and does not alter default transport refusal behavior. The Rust path keeps the authenticated Request::operation and bounded receive implementation. Neither SDK adds new identifiers, authority, scope, input replay or implicit retries. Error strings and backend response projection are unchanged.

The develop-sync additions are test-only event timestamp captures plus tests proving that a changed delivery state still fails exact response comparison (`api/crates/layer1-public/src/rest/public_contract_tests/agent_event_cases.rs:201`); the test module is cfg(test). The contract inventory adds source references without removing required problem cases. Documentation and public contract descriptions do not widen server access.

Independent local validation:

- TypeScript shared recovery replay: **33 passed**.
- TypeScript outcome/refusal tests: **18 passed**.
- Rust SDK shared recovery replay: **33 cases passed** across all five corpora.
- Rust SDK outcome/refusal tests: **3 passed**, exercising admit and resume across exact known-abort, ordinary conflict metadata, malformed/extra-field bodies and mismatched status/outcome pairs, with one-request assertions.
- Real backend dry-run replay: **4 cases passed against the real handlers**, including the exact revision assertion and all refusal cases.
- Canonical file-size gate: **passed, 6,437 source files checked**. Changed production SDK files are at most 488 lines. Rust SDK formatting: **passed**.
- Cargo tests used `lockf -k` on an existing shared heavy lock, CARGO_BUILD_JOBS=2, RUST_MIN_STACK=33554432, and the single reusable worktree target. No Mongo container or production endpoint was used.
- At both initial and final GitHub checks, the PR head matched the target and no job had failed; TypeScript SDK, repository structure, secret scanning, manifests and both Trigger contract jobs had passed. Other jobs were pending/running. This verdict does not wait for PR CI.

Evidence: `notes/reviews/2026-11-12-segmentation-integration-sec-delta-05a309059/` contains the reviewed file SHA-256 manifest, commands, exit codes and logs. All 15 reviewed file hashes were verified again after validation. Temporary exact-head export, npm dependencies/cache and runner were removed (0.21 GB reclaimed); one reusable Cargo target remains. The report is mode 0600 and evidence directory mode 0700. Source, production and GitHub state remain unchanged; no commit, push or merge was made.

VERDICT SEG-INTEGRATION-SEC 05a309059: go, 0 blockers
