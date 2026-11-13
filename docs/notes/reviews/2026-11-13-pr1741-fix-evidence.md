# PR #1741 correction evidence

Evidence checkpoint: 2026-11-13. This is a correction record, not the final GO
verdict. Completed integration results and the qualified source verdict are in
the [candidate review](2026-11-13-pr1741-candidate-review.md).
The completed [original-head primary review](2026-11-13-pr1741-primary-review.md)
records eight blockers after reviewing all 126 changed files. The original PR head is
`821790b422b336684256abbf2b00d30386f40e99`.

The recovered security review is
`a868a2ab0:notes/reviews/2026-11-12-pr1741-sec-review.md`; it reviewed
`8d318edc`, not the original PR head above. The unpublished primary report is
unavailable. The authorized primary list is the five correctness findings
below plus the shared period-counter hotspot. The independent exact-head review additionally identified B7 and B8 below;
the five recovered security findings overlap B1–B5.

## Correction status

| Finding | Change and evidence | Status |
| --- | --- | --- |
| B1: stale closure persistence | Four deterministic Mongo interleavings capture evidence before a lift, observation correction, liability release, or policy-only change. All four fail against the prior persistence path. Entries, lifts and refreshes now share a transaction guarded by policy and period revisions; refusal persistence first aborts admission. | Commit `9727d6d81`; all four races pass in the 62/62 run. |
| B2: cached STOP authority | The old maintenance path schedules a platform pause without current STOP authority. New cancellation receipts and platform pauses now fence current policy and STOP in their committing transaction. Existing exact-turn receipts survive lifts. | Commit `07bdc3b08`; pause and corrected cache baselines reproduced; all guards pass in the 62/62 run. |
| B3: missing or stale evidence | Admission derives closures from current policy, clock and period evidence; reads never lift persisted closures. New missing-period and threshold tests fail before and pass after. | Commit `04818783e`; focused Mongo run 27/27 passed. |
| B4: requests without valued holds | An active platform meter refuses missing owner valuation at command, turn and auxiliary request boundaries. Actual HTTP title adapter verifies zero upstream sends on refusal. Historical bypass remains only with the global meter off. | Commit `1b05f4506`; both new Mongo scenarios fail before and pass after. |
| B5: recovery during STOP | Failed-runtime recovery checks current platform admission state in its transaction. The test fails before by admitting a replacement runtime after a pause failure; after the fix it refuses until lift and preserves retained home and queued work. | Commit `af169e314`; regression and surrounding recovery checks passed. |
| B6: shared period-counter hotspot | Optional sixteen-counter accounting removes routine root writes while preserving exact admission under the settings fence. Legacy holds, OFF recovery and closure revision guards remain supported. | Root-write regression reproduced; corrected counter selection 74/74. See [counter evidence](2026-11-13-pr1741-counter-evidence.md), including the admission trade-off and default-OFF decision. |
| B7: queued turn after owner policy removal | Current platform admission is checked for every retained reservation, even after its owner policy disappears. | Commit `25adbd3cd`; real worker baseline sends once instead of zero; corrected 74/74 run passes. |
| B8: exact-key Revive recovery during STOP | Current authorization and fingerprint checks precede new-commitment refusal in both stores. Identical retries recover their durable receipt; new keys and changed fingerprints remain refused. | Baseline reproduced; commit `88d0bdb9b`; all replay, authorization and refusal cases pass in the 222/222 inclusive selection. |

`7cd25987c` separately extracts the evaluator persistence boundary without
changing behavior. The initial develop merge is `ac4b009f5`. All these commits
are local at this checkpoint; no fixes have been pushed to the PR branch.

## Baseline and verification method

All Mongo tests use disposable, randomly named and labeled fixtures with an
owned Docker volume, loopback-only port and cleanup trap. The image is pinned:

`mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c`

The host uses the repository-pinned Rust 1.96.1 and nextest 0.9.151. Heavy
commands run under the shared build-slot wrapper, from `api/`, with
`RUST_MIN_STACK=33554432`. No production database or cluster is involved.

The baseline method adds the new regression tests while retaining the previous
production implementation, then exercises the same persistence or provider
boundary after the fix. Intermediate runs use instrumented working trees;
they are not represented as complete exact-commit qualification. Final gate
results and their exact source boundaries are recorded in the candidate review
linked above.

Representative commands inside the disposable fixture:

```sh
cargo nextest run -p cma --locked --run-ignored only --test-threads 1 \
  --no-fail-fast -E 'test(stale_evaluator_)'
cargo nextest run -p cma --locked --run-ignored only --test-threads 1 \
  --no-fail-fast -E 'test(platform_) or test(budget_tests) or test(request_budget)'
```

The four valid B1 baseline failures are:

- `stale_evaluator_cannot_reenter_after_completed_lift`
- `stale_evaluator_cannot_close_after_observation_correction`
- `stale_evaluator_cannot_close_after_liability_release`
- `stale_evaluator_cannot_close_after_policy_change_without_ledger_write`

Each reaches the assertion that the stale decision must be discarded. The
policy-only case leaves the period revision untouched, proving a period CAS
alone is insufficient.

The valid B2 pause baseline is
`stale_stop_pause_request_cannot_pause_without_current_authority`: the old path
returns `Scheduled` where `NotEligible` is required. The initial cache test
failed during setup because its fixture omitted required owner-policy data;
that failure is not counted as defect reproduction. With the fixture corrected,
`stale_stop_cache_cannot_create_a_new_turn_receipt_after_lift` fails at the
intended assertion with the transactional guard removed: the old path returns
a platform STOP instead of no STOP for the new turn. Restoring the guard makes
it pass and preserves the prior turn receipt.

The broader B3/B4/B5 run selected 67 Mongo contracts: 65 passed and two pause
fixtures failed during that same invalid setup. Both B4 cases, the B5 regression
and surrounding recovery contracts passed. This run is not reported as green.
The subsequent corrected B1–B5 selection passed **62/62** Mongo tests. The
selection differs from the broader 67-test run; neither count substitutes for
the final full repository contract gate.

The Mongo contract selector now includes the new budget persistence and recovery
suites. Its first harness run passed 36/40 checks; four failed because the
expected prefix list had not yet been updated. After aligning the expected list,
all four failed selection checks passed. No assertion was loosened. The file-size
gate passed across 6,483 files; formatting and whitespace checks passed at this
checkpoint. `cargo clippy -p cma -p layer2-core --all-targets --all-features
--locked -- -D warnings` passed. The full repository Mongo script was pending at
this intermediate checkpoint; its completed 1,092/1,092 run is recorded in the
candidate review.

## Exact-key Revive recovery decision (B8)

On 2026-11-13 the lead confirmed that exact-key, same-fingerprint recovery must
return the original durable receipt during STOP after current resource
authorization is rechecked. Recovery creates no new commitment or spend.
The migration contract's guarantee that exact replays remain available and the
HTTP `sandbox_command_unavailable` instruction to “Retry the same key to recover
an unacknowledged receipt” govern this behavior.

This supersedes [implementation NOTES, round 3/R7](../../plans/platform-spend/NOTES.md),
which said: “An exact retry of a `Revive` admitted before the stop is refused too
(the receipt stays readable by id).” An unacknowledged caller may not know that
server-generated ID. The historical note is now explicitly marked superseded.

The service carries the platform decision into the persistence boundary. Both
stores first validate current authorization and perform the exact-key lookup;
matching fingerprints return the original receipt. A mismatched fingerprint
returns `IdempotencyConflict`, and a new key receives the original platform
budget refusal before any control record is inserted. The budget result is not
persisted and does not change the durable record schema.

The regression
`an_admitted_revive_receipt_is_recoverable_by_key_during_stop` fails before the
fix with `Budget(PlatformExhausted)` on the identical retry. Its fixture admits
while open and then enters STOP; it also asserts new-key refusal,
changed-fingerprint conflict, and unchanged records, Sandbox state and events.
`a_revive_replay_during_stop_still_requires_current_resource_authority` removes
the shared caller's membership before replay and requires denial.
The real Mongo service/store test
`platform_stop_preserves_exact_revive_receipts_and_refuses_new_or_changed_keys`
checks the same retained receipt and refusal cases with one durable record and
unchanged Sandbox state. All three tests passed in the corrected **222/222**
inclusive selection on 2026-11-13. That run includes the surrounding in-memory,
Mongo and HTTP command contracts, B1–B7 corrections and counter/census checks.
Its log SHA-256 is
`cb5f469a1626e54e58142e88e606c8892fe4530aca45d1c851d65b53f72561a2`.

The baseline log fingerprint is
`54db09c1ea7850b8fc9ba50459122b301fee457a805640f0f5c97fbc2ce3b19a`.
B7's real-worker baseline fingerprint is
`60b717f3a0e77d6e119172730ddbbce1cc1f0bf3289a35cc2787849efb6e72e7`;
it sends once where the regression requires zero, and passes in the corrected
74/74 Mongo selection.

## Persistence authority cross-check

MongoDB's [transaction production considerations](https://www.mongodb.com/docs/manual/core/transactions-production-consideration/#in-progress-transactions-and-stale-reads),
retrieved 2026-11-13, state that snapshot reads can return stale data and that an
actual modifying write acquires the document lock. Its write-conflict section
states that a transaction aborts when a document it later modifies changed
outside its snapshot. B1 and B2 use actual writes to the policy and accounting
documents; they do not assume snapshot reads alone are serializable. The local
Mongo 8.0.13 lift and disable tests independently verify this behavior against
the pinned server used for the contracts.

## Delta from the recovered review to the original PR head

Reviewed `git diff 8d318edc..821790b4` in full: 11 files, entirely from the develop
merge. It introduces bounded asynchronous Mongo diagnostic output, updates its
correlation/privacy tests, routes latency and reporting-circuit logs through
that queue, and documents possible loss and queue delay. It does not change the
platform budget or STOP paths, so it does not resolve B1–B5 or the hotspot.

The queue uses nonblocking enqueue, a fixed capacity, atomic loss counts and a
separate writer. Diagnostic records retain allowlisted metadata rather than
command/reply bodies. Circuit decisions and metrics remain synchronous.
Tests cover a stalled/full queue, disconnected writer, subscriber handling and
physical-hold correlation. The documented limitation remains: a stalled sink
can delay the loss report and abrupt exit loses queued diagnostic output.

The original PR's failed Mongo job was the terminal-settlement transaction-count
contract; the backend aggregate failed because that job failed. Merging develop
includes the default-off adoption-dispatch correction. The same settlement
contract passed in the 27-test local run and the complete integrated Mongo run.
The candidate review records the exact source verdict and separates the old
remote CI status from the unpublished corrections.

## Inclusive correction checkpoint

The corrected selection passed **222/222** across `cma`, `layer2-core` and
`layer1-public`, including ignored Mongo contracts, B1–B8, counter accounting,
archive classification and HTTP command behavior. The selector was:

```sh
cargo nextest run -p cma -p layer2-core -p layer1-public --locked \
  --run-ignored all --test-threads 1 --no-fail-fast --success-output final \
  -E 'test(platform_) or test(budget::) or test(budget_tests) or test(request_budget) or test(sandbox_controls_tests) or test(sandbox_command_http_)'
cargo clippy -p cma -p layer2-core -p layer1-public \
  --all-targets --all-features --locked -- -D warnings
```

Clippy passed, as did formatting, whitespace, deployment/configuration docs and
source-size checks. The latter checked 6,493 files. This working-tree checkpoint
precedes final integration with updated develop and #1754 and does not substitute
for the full Mongo contract script or final exact-head qualification.

## Private raw evidence fingerprints

Only sanitized conclusions and test names are committed here. Raw local logs
are temporary supporting evidence; their fingerprints bind this checkpoint.
They contain fixture diagnostics and are not production evidence. Latencies
from a shared development host are not production performance claims.

| Local log | SHA-256 |
| --- | --- |
| `b3-before.log` | `60c283945389f4bdf3833bd1ddf3f124a1b208069994fcf6d3303a2dbf698882` |
| `b3-after.log` | `9b25420aa361b592315a48bd920c2eb571adb939c5235ece7b6081021d95074b` |
| `b45-before.log` | `29bc22437b022c0b14a46ccbef93bd03ec840f23d533c382df30a24cc6baa5b1` |
| `b345-after.log` | `837f5f71450f73bd5352083b8297150322d6c4f68d3b1ba9285144d22877d5f3` |
| `b1-before.log` | `6630e08d5d12771a1df7402867d0e6b22ae8d0b1592fc11c7842f02d673a5f3d` |
| `mongo-script-tests.log` | `f190ef973f9697a4798ac914093c94704bac19bf94cbc14efc55dff7d20aa770` |
| `mongo-script-selection-after.log` | `0fc287461b5ba2124167af3c4b19aa87e9f5d3457a239a6d8fa1b052ce6f432d` |
| `b2-cache-before.log` | `8321523f3e0ccc6152c95e05ad852b6377bb4cf22106e4d2da01da77db5f1a2c` |
| `b12345-after.log` | `88013a9d4d8213eedf9780f1f4c0d2e4588171e261d57b5ca97da247abccf4fb` |
| `security-clippy.log` | `eb33ec76e6b5ee850f3c391da14393fd4679e20fe895adeb394cf09fc1ad9253` |

The original full-diff review, correction evidence and integrated qualification
are complete. The candidate review records the GO verdict at
`095cbab547eeec8e29a97f69822cd1e1f96c3817`, the complete backend checkpoint and
final integration gate. PR #1816 remains stacked until #1741 has an authorized
merge. Production activation and release qualification are separate decisions.
