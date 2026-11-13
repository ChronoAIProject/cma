# #1816 original-head review preparation

Source: `192e7884814d062a8df92be70ba0bb2988bef33c`.
Original base: `821790b422b336684256abbf2b00d30386f40e99`.
This records findings at the original PR head before integration. The final
primary review must cover the actual integrated head, reproduce the defects
there, and record the tests for its corrections. No verdict for an integrated
candidate is claimed here.

## Confirmed SDK contract failure

The PR adds nineteen operator admission cases to the shared public contract
corpus without implementing the corresponding Rust SDK operations. The existing
Rust corpus dispatcher routes `/admin/` to `operator::dispatch`, whose match
has no admission routes and panics on the first new GET operation.

Reproduction used unmodified original-head SDK source and corpus from
`git archive`, archive SHA-256
`71bf72c8f702dfa26740b85f1598425a89aada516ec6877422d428fa247afc61`.
With `CMA_SDK_CONTRACT_FILE=admin.json`, `RUST_MIN_STACK=33554432`,
`cargo test --locked --test public_contract -- --nocapture` under build-slot,
compilation succeeded and the test exited 101:

```text
thread 'shared_backend_corpus_replays_through_typed_sdk_methods' panicked
at tests/public_contract/operator.rs:122:14:
unmapped operator operation GET /api/v1/admin/admission
```

Log SHA-256:
`1ffa884fdf2f2c2d78550b5e0d8bbe1f228d053b9b11d37d10179a993636eb1b`.
The fix must supply typed read/stop/resume methods and wire dispatch; the two
blank-reason cases must exercise real local validation. Excluding these cases
would conceal the missing public client support.

## Revive receipt authorization race to reproduce

The original control service authorizes the resource, awaits the platform
admission read, then recovers an operator-paused Revive through
`lookup_sandbox_control`. That lookup accepts only a candidate record, with no
current resource proof or atomic policy guard. A policy revocation during the
await can therefore be missed on this receipt-return path.

The integrated solution should extend #1741's typed `new_commitment` refusal
through the already-authorized store reservation boundary. Exact key and
fingerprint replay precedes refusal only after current authorization; new keys
remain refused, and changed fingerprints conflict. Add a regression that
revokes the resource policy from the awaited admission-state read, covering
both operator and spend STOP. The existing test that revokes membership before
the call does not exercise this later race.

## Accepted PR-A scope

The lead explicitly approved #1816 as PR-A of #1478. Sub-issue #1849 tracks
this snapshot/read enforcement scope and is linked under #1478, with the same
labels and milestone. The parent remains open for the hard <=60-second
commit-time bound, remaining capacity work, and release qualification/drills.
No claim of that hard bound or broader issue completion is part of PR-A.

The issue's M-OPS-03 formula remains
`containment=t_last_disallowed_admit-t_switch`. Existing majority-read-then-
single-write paths and the absence of an overall database/commit deadline do
not prove a hard late-commit bound; existing tests retain that write behavior.

Do not edit the author's #1816 body and do not comment on #1478 or #1816.
The eventual #1816 merge commit body closes #1849 only. Retarget/sync follows #1741's merge; independent exact-
head review and #1816's own merge slot remain required.

## Integration invariants

Preserve #1741's current-clock fail-closed metering, mandatory valued holds,
prior-hold checks after policy removal, exact authorized replay and default-OFF
counter switch. Add operator continuation only at trusted durable-origin
boundaries; spend STOP continues to apply there. Keep the newer worker
supervision and pacing modules when resolving test-module conflicts.

The new control/history records share `platform_settings`, which the merged
retirement census already classifies Independent. Its filter selects none of
those records. No additional census key rule is needed for this storage shape.
The write gate defaults false in code and ConfigMap, while stored stops remain
enforced when writers close. Public refusal projections use fixed text and
HTTP 503 with Retry-After 60; detailed operator reason and actor stay on the
authorized admin surface.

VERDICT #1816 @ 192e7884814d062a8df92be70ba0bb2988bef33c: NO-GO (1 reproduced blocker)
