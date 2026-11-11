# Issue 1209 investigation checkpoint

This is an incomplete implementation checkpoint, not upgrade qualification.
Baseline: `d7f9cfab4`, branch `fix/1209-idle-skill-upgrade`. No runtime upgrade
capability, persisted upgrade shape, or channel binding change is enabled here.

## Original false busy result

At `677c27440aa96e428ee50f9801ea886136ea04c8` (the parent of
`c9e2d7b91`), `api/crates/layer2-core/src/sandbox/agent_live_skills.rs:172–182`
finds the skill in `work.resolved_skills`. An unequal requested version returns
`AgentOperationError::Conflict` before the idle check at line 184. The equivalent
live-installed comparison at lines 160–170 makes the same mistake. Neither
branch consults a lease, active turn, worker, or runtime.

The retained Layer 1 mapping in
`api/crates/layer1-public/src/rest/instance_problem.rs:155–159` maps `Conflict`
to `OperationInFlight`; lines 278–283 emit HTTP 409
`agent_operation_in_flight`. This explains why a different skill ID could be
installed while changing a profile-resolved version consistently failed.

Develop replaces those version comparisons with the named
`SkillVersionUpgradeUnsupported` refusal. It still does not implement upgrades.

## Remaining path audit

Paths below are under `api/crates/` and line references describe the baseline.

| Boundary | Observation |
| --- | --- |
| REST | `layer1-public/src/rest/agent_skills.rs:163–170` checks idle and install support. These return `agent_not_ready`, not `agent_operation_in_flight`. Older runtime support is currently conflated with readiness. |
| Idempotency | `layer1-public/src/rest/idempotency.rs:263–279` records refusals as completed outcomes. Replaying a key returns that outcome before consulting current Agent state (`:255–258`, `:299–305`). A cached historical false-busy response can therefore survive the code fix until its retention expires. Busy-to-idle retry using the same key needs a skill-specific admission/recovery design; it is not fixed by this checkpoint. |
| Layer 2 | `layer2-core/src/sandbox/agent_live_skills.rs:168–169` still returns `Conflict` for a different target while a live installation is pending. That Agent is not idle under `idle()`. Ordinary idle admission retries store conflicts eight times and formerly returned `Conflict` at line 261 even if no operation owned the Agent. This checkpoint returns `Unavailable` on exhaustion. |
| Parent admission | `layer2-core/src/sandbox/agent_operation_service.rs:208–223` allows ordinary input at revival/rotation `PublishActive`. Skill store admission requires Active or CredentialRotating (`layer2-core/src/sandbox/store/agent_conversation.rs:64–70`). Previously an idle child at this boundary exhausted all retries and falsely reported busy. This checkpoint checks the exact supported parent status before committing. |
| Store CAS | `cma/src/store/agent_conversation_store.rs:259–281,284–310` checks revision, generation, sequence and immutable fields, with the exact skill-admission exception. The in-memory store has the same exception. No existing skill-version update can pass the validator, which requires a new, distinct ID. CAS must remain enforced. |
| Worker | `cma/src/sandbox/agent_worker/live_skills.rs:117–166` calls the runtime only after durable admission/material pinning. Runtime errors can produce a failed row or retry after the POST has returned 202; they cannot directly produce that POST's false 409. |
| Runtime identity | `layer3-adapter/src/agent_skill_install.rs:17` fixes the identity to `agent:{agent}:skill:{id}:install:v1`. Reusing it with another version changes the hash. `layer4-runtime/src/mutation_authority.rs:207–224` refuses changed resource identity; `mutation_journal.rs:333–345` refuses changed replay material. The filesystem also refuses an existing destination. Removing only the Layer 2 refusal would not implement an upgrade. |

The two new core regressions exercise exhausted CAS with an unchanged idle
Agent, successful retry after contention, one admitted row/log, and the ready
child/parent-publication mismatch with zero attempted commits. A Layer 1 mapping
test confirms exhaustion projects to 503 `instance_service_unavailable`.
No public schema or persisted shape changes are introduced by this fix.

## Pinned native proof blocker

Local environment: macOS arm64, Node `v26.13.0`, npm package
`@openai/codex@0.147.0` reporting `codex-cli 0.147.0`, and
`@agentclientprotocol/codex-acp@1.4.0` patched in the same order as
`infra/runtime/Dockerfile`. The resulting bridge SHA-256 is
`ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66`, matching
the runtime Dockerfile. The fixture serves model responses over loopback;
no model credentials or external model calls are used. This is not a runtime
image/digest qualification or a Linux filesystem test.

The extended `layer3-adapter/src/codex/live_skill_install.test.cjs` stages a
same-name replacement and uses macOS `renamex_np(RENAME_SWAP)` (Linux
`renameat2(RENAME_EXCHANGE)` in the test's other platform branch). It keeps the
existing bridge and conversation and asks for the next model response.

Observed in three baseline runs:

- The new description is present in the third model request.
- The old description remains in `input[4]`, a historical developer message.
- The new catalog is in `input[7]`, also a developer message.
- The strict assertion that the entire request excludes the old description
  fails. The acceptance proof is **not passed**.

Two additional isolated diagnostic runs fail the same assertion: one explicitly
calls native `skills/list` with `forceReload: true`, and one uses the repository's
existing instruction refresh to unsubscribe/resume the same native thread in
the same process. Neither removes the historical developer message. The latter
is a diagnostic only, not a proposed upgrade implementation.

Local reproducible invocation after installing and patching the fixture packages:

```sh
CMA_CODEX_TEST_BINARY=/private/tmp/cma-1209-proof/node_modules/.bin/codex \
CMA_ACP_TEST_BRIDGE=/private/tmp/cma-1209-proof/node_modules/@agentclientprotocol/codex-acp/dist/index.js \
CMA_SKILL_PROOF_OUTPUT=/private/tmp/cma-1209-proof/requests.json \
node --test api/crates/layer3-adapter/src/codex/live_skill_install.test.cjs
```

`CMA_SKILL_PROOF_OUTPUT` is a test-only optional dump of synthetic fixture
requests. The strict proof remains failing for review; it must not be relabelled
as proof of the requested capability. The acceptance decision is whether old
catalogs may remain in retained conversation history while the latest catalog
contains only the replacement, or whether all old descriptions must be absent
from the entire model request. The latter has not been demonstrated with the
pinned provider. No transcript rewrite or new conversation was attempted.

`docs/CMA_Data_Migrations.md` is absent from this baseline and from the locally
available Git history. `CMA_Migration_Matrix.md` and `CMA_Migration_Runner.md`
still link to it. Before admitting a new persisted upgrade shape, the governing
migration document and compatible recovery-reader floor must be resolved.

## Checkpoint validation

Every Cargo command ran from `api/` after
`source /private/tmp/cma-gates/env.sh`, with toolchain 1.96.1 and four build jobs.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed. |
| `cargo nextest run -p layer2-core --locked -E 'test(skill_contention_tests)'` | Both new core regressions passed. |
| `cargo nextest run -p layer1-public -p layer2-core -p layer3-adapter --locked` | 2,609 passed, 5 skipped. Includes the REST mapping and public-contract tests. |
| `cargo clippy -p layer1-public -p layer2-core -p layer3-adapter --all-targets --all-features --locked -- -D warnings` | Passed. |
| `cargo test --doc -p layer1-public -p layer2-core -p layer3-adapter --locked` | Passed; these crates contain zero doctests. |
| `.github/scripts/check-file-size.sh` (repo root) | Passed, 5,775 files checked. |
| `.github/scripts/check-config-keys.sh` (repo root) | Passed, all three service comparisons. |
| `git diff --check` | Passed. |
| Native proof command above | Failed at the strict old-description exclusion; latest-catalog replacement assertions passed. |

Local logs and synthetic request evidence are under
`/private/tmp/cma-1209-proof/` (`rust-tests.log`, `clippy-focused.log`,
`doc-tests-focused.log`, `file-size.log`, `config-keys.log`, `live-upgrade.log`,
`refresh-tests.log`, `requests.json`). They are not committed generated artifacts.
Whole-workspace Clippy, nextest and doctest gates have not run at this incomplete
checkpoint. SDK/web checks were not run; neither package was changed. Part 2's
durable upgrade, capability/reader-floor rollout, and its crash-point tests are
still unimplemented. The binding PUT assessment remains deferred until part 2.
