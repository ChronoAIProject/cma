# Bounded integration quality repairs — 2026-11-03

Branch `codex/cma-ui-quality-repairs`, worktree
`/Users/liuyitian/code/cma-ui-worktrees/quality-repairs`.
Original comparison baseline: `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706`.
Integration base: `9bc2f04e883c6619e65ad7f424994f333c385055`.
Validated source commit: `8ab9c2deeb4ea51e169d15e5498c06e3cc364c76`.
Parent thread: `01a19c3f-df8b-72a3-8927-fe95218015c6`.

## Changes and exact attribution

| Finding | Task origin | Repair |
| --- | --- | --- |
| `AnswerAuthority` stores 7,992/6,368-byte variants inline | Ticket 07, `13e6f72b9b99cf3aea8730ccecb45d4fe2c9f705`; file absent from original baseline | Box the shared authorization and personal lineage; retain the same caller, snapshots and revalidation logic. |
| Delivery `access` / `sequence` return a 128-byte HTTP error response | Ticket 07, same originating commit | Return existing typed errors and perform the identical response conversion in the handlers. |
| Feedback-reasons `sequence` returns a 128-byte HTTP error response | Ticket 10, `f32906d1b636bbab1c01e446c01d577fb72ea861`; file absent from original baseline | Keep `InstanceProblem` until the handler converts it. |
| Filesystem runtime fixture loaded twice; duplicate copy has unused browser fields | Delivery corpus addition `5d957af0440303abdf2cdf20fc23146f0a30632b`, followed by browser fixture wiring `42f52fe43` | Declare the fixture once in the common test parent and import it from both consumers. Its unchanged `kind` gate still exposes no browser for delivery mode `owner`. |
| SDK suggestion validation has nested collapsible conditions | Ticket 13, `d0cff82a00816d0112899ab25256a1d72cbec00a` | Use an edition-2024 let chain, preserving every validation predicate and error. |
| Six redundant SDK corpus borrows | `5d957af0440303abdf2cdf20fc23146f0a30632b` changed `for case in corpus` to `for case in &corpus`; the older call sites consequently became double references | Pass the existing case reference directly. |
| Newly added SDK tests leave shared wire-harness members unused | Ticket 13 suggestions and ticket 15 `1ff50332535573815db16ec97ec7a3d08ca881ed` browser test; the harness itself is unchanged from the original baseline | Assert exactly two suggestion GETs; exercise the existing browser identity checks with anonymous, authenticated and rebuilt clients, including exact auth headers and six total GETs. |

No warning allowances, skipped assertions or storage/wire schema changes. The
existing public delivery tests verify the storage refactor; no boxing tests were
added. The SDK test assertions remain at the already-approved public client seam.
No preserved original defect, ticket 16 creation code, ticket 17 control code,
production environment, shared cache or original working file was modified.

## Isolation and lint evidence

All raw evidence is outside the repository in
`/Users/liuyitian/code/cma-ui-worktrees/evidence/quality-repairs/`.
Rust/Cargo are actual **1.96.1**, `CARGO_BUILD_JOBS=2`,
`RUST_MIN_STACK=33554432`, with `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` unset.
API edition is 2021; SDK edition is 2024.

The API target is the real private directory
`/Users/liuyitian/code/cma-ui-worktrees/cargo-target-quality-repairs`.
It was APFS-cloned from integration's target, then all nine API workspace
packages were privately cleaned: **39,663 files / 58.6 GiB** removed.
Logs confirm recompilation from this worktree. The SDK uses a fresh private
`/Users/liuyitian/code/cma-ui-worktrees/cargo-target-quality-repairs-sdk` target.
No other worktree's target was cleaned or used for writes.

| Command, from the respective workspace | Result | Raw log |
| --- | --- | --- |
| API: `cargo +1.96.1 clippy --offline --locked -p layer1-public -p layer2-core -p layer3-adapter -p layer4-runtime --all-targets -- -D warnings` | **exit 0** | `four-layer-recheck.log` |
| API, after the four-layer pass on the same source: `cargo +1.96.1 clippy --offline --locked -p cma --all-targets -- -D warnings` | **exit 0** | `cma-clippy.log` |
| SDK: `cargo +1.96.1 clippy --offline --locked --all-targets -- -D warnings` | **exit 0** | `rust-sdk-clippy-final.log` |

The original enum failure was independently reproduced in `four-layer-red.log`
(exit 101). Subsequent genuine failures are retained in `four-layer-green.log`
(typed helper errors), `four-layer-final.log` (duplicate fixture), and
`rust-sdk-clippy.log` (new test wiring); those intermediate filenames are not
success claims. The parent SDK red is `../rust-sdk-clippy-after15.log`.
`rust-sdk-suggestion-harness-red.log` separately reproduces the latent unused
counter/method on the original suggestion test without the count assertion; the
committed test was restored byte-for-byte before the final SDK lint.

## Existing behavior checks

All tests below execute against the private target with `--offline --locked` and
`--nocapture`; corpus recording/update modes are disabled. No ignored test is
counted as a pass.

| Check | Actual execution | Raw log |
| --- | --- | --- |
| L1 `rest::answer_deliverables_tests::` | **5 passed**, zero failed/ignored | `public-deliverables.log` |
| L1 `message_feedback_reasons_` | **5 passed**, zero failed/ignored | `message_feedback_reasons_.log` |
| L1 `public_files_` | **2 passed**, zero failed/ignored | `public_files_.log` |
| L1 exact `rest::public_contract_tests::shared_public_contract_matches_real_backend`, `CMA_PUBLIC_CONTRACT_FILE=answer-deliverables.json` | **1 passed / 44 handler cases** | `public-answer-deliverables-corpus.log` |
| Same exact test, `feedback-reasons.json` | **1 passed / 26 handler cases** | `public-feedback-reasons-corpus.log` |
| Same exact test, `agent-browser.json` | **1 passed / 36 handler cases** | `public-agent-browser-corpus.log` |
| Same exact test, `agent-filesystem.json` | **1 passed / 98 handler cases** | `public-agent-filesystem-corpus.log` |
| Same exact test, `agent-environment.json` | **1 passed / 10 handler cases** | `public-agent-environment-corpus.log` |
| SDK `cargo test --test suggestions --test agent_browser` | **1 + 1 passed**, zero failed/ignored | `rust-sdk-focused.log` |
| SDK exact `shared_backend_corpus_replays_through_typed_sdk_methods`, `CMA_SDK_CONTRACT_FILE=answer-deliverables.json` | **1 passed / 41 typed HTTP + 3 typed preflight cases** | `rust-sdk-answer-deliverables.log` |
| Same exact test, `agent-browser.json` | **1 passed / 36 typed HTTP cases** | `rust-sdk-agent-browser.log` |

Delivery checks include
`answer_deliverables_keep_two_published_results_separate_after_reconnect`,
`answer_deliverables_download_the_original_document_revision_after_an_edit`,
`answer_deliverables_require_independent_document_access_and_honor_revocation`,
`answer_deliverables_keep_explicit_repeats_but_reject_examples_and_nonfinal_messages`,
and `answer_deliverables_report_bounded_turn_reads_as_unavailable`.
SDK behavior names are
`saved_suggestions_use_only_a_scoped_get_and_refuse_a_different_answer` and
`browser_reads_are_bounded_and_bound_to_the_requested_agent_incarnation`.
The first overly specific feedback module filter selected zero tests; its
`public-feedback-reasons.log` is retained but excluded from evidence. The
corrected filter above executed all five named tests.

## Handoff boundaries

Owned Rust formatting and diff checks pass. Every changed source is below 500
lines and every changed test below 2,000. All **19 tickets / 149 original
acceptance-criterion texts** match the original baseline exactly, excluding only
checkbox status; no ticket or its acceptance status was edited by this repair.
`acceptance-criteria.txt` retains that audit. Original full-format, skill-fixture,
508-line Agent service and LocalAgentV2 head-404 limitations remain outside this
repair; the targeted checks above encountered none of those failures.

Final committed file SHA-256 values, exact commit, commands, named counts, test
binary hashes and raw-log hashes are in the external `manifest.json`. Latest
integration is merged before handoff. No full SDK suite, native runtime, Chrome,
Mongo or production proof is claimed here; those unchanged capabilities retain
their previous evidence and the parent's independent SDK validation.
