# Admission 10% B2 exact-manifest delta review — 2026-11-12

VERDICT ADMISSION-487c44a3: go, 0 blockers

Reviewed exact manifest `487c44a37ec74676d70ec28bcc9d2d36aa13add89739b0c832deeb29d96f09bb`, superseding `5a53e116d6793332d964a2903f4307a279f0f4a7e7c8e9b577b16926f9e5eb7f`. The requested manifest SHA was checked before inspecting the delta. All 138 bound files and the packet plan were verified, then reverified before this report. Packet plan SHA-256: `107f143c7e6254776b1e5b175a73cafdf0abc9c539cbfde65d6023f4cd96dd56`. The implementation handoff still matches `5d70f9028f2230ac6d651954ca9aa7833c88024357aaf07aa63584899a41ce0e`.

This is a GO for the exact reviewed controller delta. It does not certify future live prerequisites, authorize a different image or percentage, or claim that activation/rollback has executed.

## B2 closed

`controller/disable_policy.py:8` defines the comparison of managed container fields: image, env, envFrom, command, args and resources. Both the Pod-to-Deployment comparison (`:24`) and the ReplicaSet-to-Deployment comparison (`:32`) use it. Kubernetes-injected volumeMounts, service-account fields and unrelated container defaults are excluded. Image equality remains exact; the surrounding unchanged preflight pins it to the immutable reviewed digest. Both admission keys remain protected, together with the entire explicit env list and inherited envFrom references. This retains stricter environment checking than comparing only the two keys.

The captured generation 183 fixture has different Deployment and Pod container structures, with volumeMounts as the only differing field. The new test at `controller/test_disable_policy.py:151` accepts that shape and captures effective zero. The test at `:165` separately changes image, env, envFrom, command, args and resources; every case refuses. The earlier B2 failure is therefore removed without dropping the selection or image checks.

## B1 remains closed

The `enabled_replicas` function is unchanged in its abstract syntax from the prior reviewed B1 fix. `controller/disable_policy.py:54` still requires the live absent-key ReplicaSet to be in the captured prior inventory with exactly one proof. The proof must match source, deployment ownership, effective 0/empty selection, full ReplicaSet template hash, immutable image and unchanged resolved-reference metadata (`:56`–`:63`). Known zero-percent processes may remain live or drain while disable proceeds; unknown or changed live processes refuse. Positive-percentage ReplicaSets remain in the exit-proof inventory.

The B1 tests still cover retained and draining prior replicas, unknown/missing/duplicate/changed proofs, invalid selection and incomplete enabled inventory. The actual disable-controller test still exercises dry-run, same-image zero-percent patch, readback and recovery binding while the original zero-percent Pod drains.

## Mutation, rollback and verification are preserved

Hash comparison confirms no changes to `stage_patch.py`, `stage_disable.py`, `train_policy.py`, `disable_recovery.py`, `phase_binding.py`, `review_gate.py`, `bridge_check.py`, `bridge_custody.py`, `release_rollback.py`, `admission_prerequisite.py`, `verify_api.py` or `after_controller.py`.

Consequently the initial same-image 0→10 env-only patch retains full UID/resourceVersion/generation/spec/container tests, exact dry-run comparison, writer custody, a single-attempt marker, response/readback equality and no blind uncertain retry. The kill switch retains same-image 10→0 plus explicit empty selection and the same fences/readback. The complete Pod/ReplicaSet/endpoint exit proof still refuses mixed or incomplete inventories; applied rollback remains unverified until all enabled processes are gone and its health/continuity/observation gates pass. The B2 helper adds no mutation or recovery path.

## Exact delta and validation

Compared the new manifest against the retained previous manifest after verifying its original SHA. Only `disable_policy.py` and `test_disable_policy.py` changed among executable/test files. The supporting `gen183-container-shape.json` fixture was added and `controller-tests.json` updated. The packet plan only appends the B2 explanation. All external-file hashes are unchanged; no controller files were added or removed. The fixture SHA-256 is `46db6b839c6372d1d3c06e5541378c2cf91a3017a0f777aaf5847ab73537e0de`.

Independently ran the complete controller suite from a temporary copy within this worktree: **98 passed, zero failures/errors**, in 2.211 seconds. This includes the two new production-shaped B2 tests and the retained B1, patch-fence, drain, custody, review-hash, safety and observation tests. No test was run against production. Results and the test-log hash are recorded in `delta-487c44a3-offline-tests.json`; the full log is retained alongside it.

Temporary copies were removed. Evidence directory permissions are 0700 and this report/evidence files are 0600. No production reads or writes were performed for this delta, no controller source was edited, and no account/grant change, build, container, commit or push occurred. The worktree remains on `tl/cma-ledger-recovery/maintenance-recovery` at the previously published `3bd324f20935347f62e1836ca0068cddc15a309c`; Track C remains paused. Findings are reported to the lead only.
