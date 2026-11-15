# #1816 final primary review

Candidate source: `ec6e6c8c20ccfdbd6f931a28ee59946a8775ac0f`, targeting `develop`.
The reviewed production source and test trees are also carried by the subsequent
evidence-only commit; its exact head is reported directly with the push request.
Scope is PR-A of #1478 and sub-issue #1849. This review qualifies the next-snapshot
operator stop and preserved admitted work. The parent remains open for the hard
<=60-second commit-time bound, remaining capacity work, and release drills.

## Findings and corrections

The original head was `192e7884814d062a8df92be70ba0bb2988bef33c`. Both confirmed
blockers are corrected in `277dd34cf28b218c913491f4bd364c126d2ed880`:

1. The operator-STOP receipt shortcut could return an accepted Revive after a
   resource share was revoked during the awaited admission-state read. The
   proof-free lookup is removed. Both operator and spend refusals reach the
   guarded reservation boundary, which validates current resource policy before
   fingerprint comparison and exact receipt replay. A real share-revocation
   regression failed before the fix in memory; the corrected behavior passed
   afterward in memory and Mongo.
   New keys remain refused; changed fingerprints conflict without new effects.
2. The new admin corpus had no Rust SDK operations and failed with an unmapped
   GET admission operation. Typed read/stop/resume methods now replay the shared
   contract. Reason validation uses UTF-8 bytes; history is required, bounded
   and ordered; uncertain commands are not implicitly retried. The original
   corpus failure and passing regression are recorded in the integration report.

The [integration report](2026-11-13-pr1816-integration-evidence.md) records the
before/after evidence and log hashes. The [original review](2026-11-13-pr1816-original-review.md)
records original source/archive provenance and the initial SDK NO-GO.

Reviewed the complete original change and integrated source, including capability
checks, strict history and CAS, uncertain commits, server-attributed continuation,
resource/Launch/binding/registration boundaries, retirement, exact-key replay,
public refusal projections, both SDKs, configuration and the migration contract.
No additional production blocker remains within the approved PR-A scope.

## Integration and preservation

#1741 merged at `734bc9c2ef5d3589aa25311003c69d9b94bf20a7`. Its spend checks remain
in force: current-clock metering, fail-closed missing evidence, mandatory valued
holds, prior-hold STOP after policy removal, and recovery's spend gate. Operator
continuation exempts only already-admitted work from the independent operator
stop. A regression retains operator STOP while proving recovery only after the
spend stop lifts.

Both `CHRONO_ADMISSION_CONTROL_WRITES_ENABLED` and
`CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` remain default-OFF in code and infra.
Existing stored stops and activated counter layouts remain readable. Counter
activation remains a separate measured decision: admission 97 -> 114 ms with
doubled reply bytes versus settlement p95 65 -> 14 ms. This PR activates neither
switch and provides no production release qualification.

Control and history remain in `platform_settings`, already classified Independent
by the merged retirement census; no new census key rule is required here.

The test-only decomposition in `2ce7a342d90c72369565cac9d2f3cb1ec602907e` retains
all assertions/functions and all 436 SDK classification pairs. Every source file
touched by this PR is below 500 lines. The classification-pair SHA-256 is
`f4d77d77ecfc0a3845c4d4d02bff05d27345d878939831afa37fa5e488aff0c2`.
The develop sync includes #1846's deterministic pacing fixture and #1838's
bounded Gateway refusal metadata. These changes were reviewed for interaction
with the admission changes and qualified in the affected local gates.
The final develop integration is `aa7e340cb9e8b09d06495dadadba674bd8934452`.
Across that sync, the PR source diff excluding review reports is identical:
SHA-256 `82e912a75b92dd78ddec1de5d1808626e9cdda7108012c90d56507d65cf5d20c`.
The cma, layer1-public and layer2-core trees, Cargo manifests/lockfile, CI
configuration and infra are identical before and after that sync.

## Local qualification and provenance

| Gate | Observed result | SHA-256 of private log |
| --- | --- | --- |
| Full Mongo contract suite at `277dd34c` | 1,141/1,141 passed across 33 owned fixtures; cleanup exited zero | `5564c61e8ededac381cd5efb0d30516f679d788ee00e66d7376cf394ca45b9d7` |
| Affected backend and moved Mongo contracts at `2ce7a342` | fmt, strict clippy and doctests passed; 4,141 ordinary tests passed, 1,307 skipped; 57 focused Mongo contracts passed across two bounded fixtures | `0f4a4f8d4f6fce04ba83695d199f7ac7699224d4d039af211fe1619f8a4b8e52` |
| Synced candidate backend/Gateway gate | fmt and strict clippy across the five affected crates passed; 3,352 Gateway/core tests passed, 106 skipped; doctests and complete Mongo discovery passed | `e2a2b92a1c81ca5f26884d939b4c86e6b7cac7b0854908162a651e38dde9b468` |
| Synced candidate Rust SDK | fmt, strict clippy and all-target build passed; 284 tests passed, two skipped; 38 doctests and the dry-run example passed | `e6708b0ab73bc926eaef57904a81a169df69899dc648c82910a2919fc2e43ea9` |
| Synced candidate TypeScript SDK | lockfile install, format, lint, typecheck, coverage, build, docs and package consumer passed; 4,476 tests passed, one skipped; 96.53% lines; audit has no high/critical findings, seven moderate findings | `994d0d1d104129cfd89d813c4ef06f4a57e7a34d2d3cd0d9158871acd999623d` |
| Candidate secret scan | No leaks in the branch commit range | `a84a1209c6ab17af26ed69c4c2dd91fd3ca4f3ba2b6b07b1b19096fa7e8aa1f4` |
| Candidate repository file-size gate | Passed; all 160 source files touched by this PR are below 500 lines (largest: 498) | `cc81f16be015d40db5e4039919df676233b9c6bbe6de3a9e4f2f5d47f68125fa` |

The complete Mongo selection retains 1,141 contracts after module renames.
Normalized selection SHA-256:
`8b7a2d7cb30855332124d01164c09422c26a6fd00cd1b2a031b4a18de79c872e`.
Before/after discovery file hashes are
`c9706b2401ce55deb47577724ff254a346cf0b34d997876b0f5efae2e3ce9ff4` and
`7a83719cc7edf3a1930af73006273e56f4664d1d5da6625383332e8ae1803a03`.
Final synced discovery also selects exactly these 1,141 contracts; file SHA-256
`300aae69a9d8e9ad92807184377c881586a4377415c0622d4e1f7b764222efce`.
No test was removed, weakened, skipped or re-timed to obtain these results.
The first check of the moved tests found one incorrect relative import; it was
corrected before the passing final gate. The imported pacing test change is
the separately reviewed #1846 change already merged to develop.

The Rust SDK scratch consumer also built and ran with the typed admission example
before the test-layout change; its log SHA-256 is
`a4140a279ea53ee784600d46bd9414007f027a1a4daffe2450a55658d45458f3`.

All Rust/Mongo builds and tests used the shared build-slot wrapper and
`RUST_MIN_STACK=33554432`. Mongo used disposable loopback-only replica sets from
the repository's pinned Mongo 8.0.13 image. All fixture commands exited zero,
including cleanup. No production access occurred. Raw logs stay private;
committed counts, source provenance and SHA-256 hashes identify the evidence.
No Rust coverage percentage is claimed because the local coverage tool is absent.

## Publication and remaining gates

The remote PR remains at its original head until the separately authorized normal
push. Its zero reported checks do not qualify this candidate. Independent
pr-review-b review and #1816's own merge slot are still required.

The author's PR body is unchanged, SHA-256
`8d3cf573c92c6d1df6201780a23840f58aaf81edc2f017582366fbaf4a46180a`.
No issue/PR comment was posted. The scoped sub-issue #1849 has the parent's
labels and milestone and is linked as its child. The eventual merge commit body
closes #1849 only; #1478 stays open.

VERDICT #1816 @ ec6e6c8c20ccfdbd6f931a28ee59946a8775ac0f: GO
