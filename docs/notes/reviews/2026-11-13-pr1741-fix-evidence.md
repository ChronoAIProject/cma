# PR #1741 correction evidence

Evidence checkpoint: 2026-11-12T19:19:50+00:00. This is a partial correction record, not a GO
verdict or a completed full-diff review. The original PR head is
`821790b422b336684256abbf2b00d30386f40e99`.

The recovered security review is
`a868a2ab0:notes/reviews/2026-11-12-pr1741-sec-review.md`; it reviewed
`8d318edc`, not the original PR head above. The unpublished primary report is
unavailable. The authorized primary list is the five correctness findings
below plus the shared period-counter hotspot. A fresh exact-head review must
also identify additional defects rather than assume this list is exhaustive.

## Correction status

| Finding | Change and evidence | Status |
| --- | --- | --- |
| B1: stale closure persistence | Four deterministic Mongo interleavings capture evidence before a lift, observation correction, liability release, or policy-only change. All four fail against the prior persistence path. Entries, lifts and refreshes now share a transaction guarded by policy and period revisions; refusal persistence first aborts admission. | Implementation awaiting green regression run. |
| B2: cached STOP authority | The old maintenance path schedules a platform pause without current STOP authority. New cancellation receipts and platform pauses now fence current policy and STOP in their committing transaction. Existing exact-turn receipts survive lifts. | Pause baseline reproduced; corrected cache fixture baseline and green run pending. |
| B3: missing or stale evidence | Admission derives closures from current policy, clock and period evidence; reads never lift persisted closures. New missing-period and threshold tests fail before and pass after. | Commit `04818783e`; focused Mongo run 27/27 passed. |
| B4: requests without valued holds | An active platform meter refuses missing owner valuation at command, turn and auxiliary request boundaries. Actual HTTP title adapter verifies zero upstream sends on refusal. Historical bypass remains only with the global meter off. | Commit `1b05f4506`; both new Mongo scenarios fail before and pass after. |
| B5: recovery during STOP | Failed-runtime recovery checks current platform admission state in its transaction. The test fails before by admitting a replacement runtime after a pause failure; after the fix it refuses until lift and preserves retained home and queued work. | Commit `af169e314`; regression and surrounding recovery checks passed. |
| Shared period-counter hotspot | Reservation and settlement still read and write one global period document. Budget-safe distribution, migration/rollback behavior and local measurements remain required. | Open blocker. |

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
they are not represented as complete exact-commit qualification. Final gates
will be rerun at the final candidate head.

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
that failure is not counted as defect reproduction. The fixture is corrected,
and its proper baseline is pending.

The broader B3/B4/B5 run selected 67 Mongo contracts: 65 passed and two pause
fixtures failed during that same invalid setup. Both B4 cases, the B5 regression
and surrounding recovery contracts passed. This run is not reported as green.
The corrected fixtures and B1/B2 guards require the next verification run.

The Mongo contract selector now includes the new budget persistence and recovery
suites. Its first harness run passed 36/40 checks; four failed because the
expected prefix list had not yet been updated. After aligning the expected list,
all four failed selection checks passed. No assertion was loosened. The file-size
gate passed across 6,483 files; formatting and whitespace checks passed at this
checkpoint. Clippy and the full repository Mongo script remain pending.

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
contract passed in the 27-test local run. Final CI and exact-head status still
need to be checked after all corrections.

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

The complete full-diff review, counter measurements, final verification and
exact-head verdict remain outstanding. PR #1816 remains stacked until #1741
has a reviewed and authorized merge.
