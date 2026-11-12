# Bounded admission 10% exact-manifest review — 2026-11-12

THU CONTROLLER REVIEW: NO-GO — 1 blocker at manifest `d3a3ecdaeb7f655aa38d8117900e707537bc13d9fbc219ea9130322267681017`.

This supersedes the earlier missing-packet HOLD. The supplied packet and all 135 bound files match their hashes. Plan SHA-256: `f823bbd03708724c7d110a7b3c57fe4b17626a7203ce49913c1649a2a49d1b1f`. Source: `ade94de34a3f0cc173c8bc6c607e504c342bfdf6`. Image: `sha256:859264c2f8edcb1ca7a6c8a2f159cee9aa1a17f68fa61127efae192d2a9dd5d3`.

## B1 — Kill switch refuses while the original zero-percent ReplicaSet remains live

`controller/stage_disable.py:46` inventories all owned ReplicaSets. At `:52`–`:54`, every ReplicaSet lacking a direct fleet-percentage entry must already have both desired and actual replicas equal to zero. The accepted generation 183 has both selection keys absent and effective values 0/empty; initial enrollment deliberately adds them only to the new template (`train_policy.py:96`, `stage_patch.py:90`). The prior ReplicaSet therefore necessarily lacks this entry.

During a rolling activation, the original zero-percent process can still serve while the new ten-percent process starts or fails. An early crash-loop or other safety trigger reaches `stage_disable.py:53` and aborts before constructing or applying the 10→0 patch. A draining original ReplicaSet with desired=0 but actual=1 also aborts. The implementation therefore cannot provide the promised kill switch throughout the rollout; the new selected process may continue executing while automatic disable has failed. This is particularly relevant when an unhealthy candidate prevents the prior ReplicaSet from disappearing.

Offline reproduction invokes the exact `apply_disable()` function with all preceding checks satisfied and Kubernetes/subprocess operations mocked. With the previous absent-key ReplicaSet at (desired, actual)=(1,1) and (0,1), it raises at line 53. With (0,0), it reaches recovery preparation. No Kubernetes request or patch was performed. See `rollback-overlap-repro.json` and `rollback-overlap-repro.py`.

Required correction: recognize the captured prior ReplicaSet/process as zero-percent using its hash-bound baseline identity, configuration and effective-selection proof, allow that proven-zero process to remain during same-image disable, and continue accounting for every enabled or uncertain process. Do not replace the assertion with an unconditional skip for arbitrary absent-key ReplicaSets. Add success coverage for a retained and a draining proven-zero prior ReplicaSet, and refusal coverage for unknown or conflicting selection. Publish a new exact manifest for delta review.

## Checks that passed

- `train_policy.py:82` builds the full fenced patch; `:96` permits initial enrollment only after effective 0/empty proof, and `:125` protects image equality. UID, resourceVersion, generation, full spec and container-name tests accompany the env-only change. Exact server dry-run comparison rejects additional spec/metadata changes. `stage_patch.py:29` restricts this packet to 0→10; `:88` binds the same image.
- `admission_prerequisite.py:34` requires genuine completed Web acceptance, contiguous observation/log intervals, four origin checkpoints and the rendered fixture. `:97` binds the accepted generation 183 receipt plus the separately granted admission scope. `stage_patch.py:59` requires a fresh post-Web comparator; `:126` rechecks the prerequisite before mutation.
- `stage_patch.py:151` holds local exclusion and bridge writer custody through write/readback; `:139` rechecks the window and `:141` marks the single attempt. The rollback likewise uses writer custody and refuses blind repetition after uncertain acknowledgement (`stage_disable.py:25`).
- `after_controller.py` waits through H=R+1411 seconds and a subsequent 15-minute clean interval before collecting 3,600 seconds. `verify_api.py` independently binds the steady hour, generation, source, live image, recovery, continuity and protected-state evidence. Sparse or unresolved measurements remain PARTIAL in `fleet_acceptance.py`.
- `fleet_window_policy.py` retains the two-bin route, checkout and CPU thresholds, immediate user-5xx/checkout-failure decisions, coverage holds and preparation-growth attribution. `fleet_observer.py` dispatches newly observed quarantine before optional enrichment; `release_watch_api.py` assesses known circuit/report/readiness/recovery/user-failure triggers before optional collectors. Final acceptance rechecks the assessed circuit digest.
- Once its preconditions are met, rollback is same-image 10→0 plus explicit empty selection. `disable_recovery.py` requires all enabled processes gone, an independent health boundary, clean watch and full settled hour; it does not treat patch acknowledgement as verified recovery. B1 prevents relying on that path during early rollout.

## Validation and evidence

Independently ran all 87 offline tests from a temporary copy in this worktree: 87 passed, zero failures/errors. The supplied tests do not cover the live prior absent-key ReplicaSet case; the additional reproduction demonstrates B1. `controller-offline-tests.json` binds the retained test log. All 135 manifest-bound files and the plan were verified again after validation.

The earlier GET-only observation at 2026-11-12T06:48:34.906175Z remains in `live-config.json`: API generation/observed 183/183, one Ready process, zero restarts, exact image, selection defaults 0/empty. It is historical review evidence, not a substitute for fresh execution gates. Source hashes and the earlier nine pure performance-evaluator checks remain retained.

Evidence directory is 0700 and files are 0600. Temporary controller/test copies were removed. No production mutation, controller entrypoint execution, build, container, commit or push occurred. Track C remains paused. This verdict applies only to the bound admission packet, not the independent Web review.
