# Admission 10% B1 delta review — 2026-11-12

THU CONTROLLER REVIEW: NO-GO at `5a53e116d6793332d964a2903f4307a279f0f4a7e7c8e9b577b16926f9e5eb7f` — B1 addressed; one new blocker B2 in prior-proof capture.

Baseline manifest: `d3a3ecdaeb7f655aa38d8117900e707537bc13d9fbc219ea9130322267681017`. Exact new manifest and all 137 bound files verified. New plan hash: `32d2a9059c30b4194db58bcc594bba98b8a441489579301b78e33a38fe13a403`.

## B2 — New proof capture rejects the actual production Pod

`controller/disable_policy.py:16` requires the entire live Pod API container to equal the Deployment template API container. `controller/stage_patch.py:99` invokes this assertion before writing the activation. Kubernetes has injected a read-only projected service-account volume mount into the live generation 183 Pod; that mount is absent from the Deployment template. The containers therefore differ at `volumeMounts`, and proof capture refuses before activation.

Read-only production GETs at 2026-11-12T07:18:48Z established generation 183, one matching Pod, exactly one owning ReplicaSet, and equality between the ReplicaSet and Deployment API containers. The Pod differs only in `volumeMounts`. A second read at 07:19:56Z identified the added mount as the standard service-account path, read-only, backed by projected serviceAccountToken/configMap/downwardAPI sources. Only hashes, field names, booleans and aggregate metadata were retained; no credential values were read from the volume or saved.

Offline reproduction uses the exact new `capture_prior_zero()` function and the supplied fixture. The original fixture succeeds; adding the production-shaped service-account mount to the Pod alone raises at line 16. Evidence: `delta-5a53e116-capture-readback.json`, `delta-5a53e116-injected-mount.json`, `delta-5a53e116-capture-repro.py` and its JSON result.

Correct the comparison to accommodate only the verified Kubernetes-injected service-account projection, or compare the authoritative selection/image/configuration fields while retaining the exact ReplicaSet template and ownership proof. Preserve refusal for real environment, image, reference and ownership drift. Add a production-shaped Pod fixture and publish a new manifest. The current failure is fail-closed, but this exact controller cannot execute the requested activation.

## B1 and delta scope

B1's rollback classification is corrected: `disable_policy.py:34` recognizes an active or draining absent-key prior ReplicaSet only with the captured identity, deployment owner, source, effective zero/empty selection, exact template, same image and unchanged environment-reference metadata. Unknown live ReplicaSets with absent admission configuration, missing or duplicate proof, altered template/image/owner/references, invalid selection and an empty enabled inventory still refuse. Positive-percentage ReplicaSets remain in the exit-proof inventory. The actual disable-controller test reaches server dry-run, zero-percent patch, readback and recovery binding while the original zero-percent Pod drains.

Compared manifests using the retained prior manifest after verifying its original SHA-256. Only `stage_patch.py` and `stage_disable.py` changed; `disable_policy.py` and `test_disable_policy.py` were added. No code was removed. The supporting test report changed, the plan gained only its B1 appendix, and all external-file hashes are unchanged. The same-image env-only patch, mutation fence, custody, health gate, 60-minute observation, safety thresholds and rollback observation code are unchanged.

Independently ran all 96 offline tests: 96 passed, zero failures/errors. Their synthetic Pods lack Kubernetes-injected mounts, so they do not cover B2. Temporary test/reproduction copies were removed. Evidence remains 0700/0600; no production writes, controller entrypoint execution, build, container, commit or push. Track C remains paused.
