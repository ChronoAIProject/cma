# AM successor image rollout

Refs #2244. This controller replaces the private revision-231 launcher for the next AM image rollout. It performs one image patch on the complete revision-233 spec. It never bootstraps the HPA, sets environment variables, changes lifecycle hooks, rolls back, or scales replicas automatically.

The packet is preparation-only until the final candidate, remote publication receipt, executed rehearsal template, runtime qualification, independent review, write window and single-use lead approval are bound. Revision 234 is the expected next Kubernetes revision; the controller does not set that annotation. Any revision/generation/spec drift requires a new packet and review.

Run offline validation with Python 3.9+:

```sh
python3 -m unittest discover -s .github/scripts/am_rollout -p 'test_*.py' -v
python3 .github/scripts/am_rollout/controller.py --packet /private/packet.json --out /private/preflight-new
```

The default mode only reads saved regular files. Execution is a separate future action, prohibited while the release is on hold:

```sh
python3 .github/scripts/am_rollout/controller.py --packet /private/packet.json \
  --packet-sha256 CANONICAL_PACKET_SHA256 --review /private/review.json \
  --approval FRESH_SINGLE_USE_ID --execute --out /private/unique-execution
```

The policy context is `am-rev234/<canonical-packet-sha256>`. The canonical hash is SHA256 of UTF-8 JSON with sorted keys, compact separators and `ensure_ascii=False`. Review JSON must contain `verdict: GO`, the `packet_sha256`, and SHA256 of the exact `controller.py` bytes. The lead's actual policy checker independently validates and consumes the approval; local review metadata is not an approval credential. The complete review must be read before running reviewed tooling.

The packet schema is `am-successor-v1`. It has `context`, `namespace`, `execution_enabled`, `write_window` (`start_epoch`, `end_epoch`), and these bindings:

- `baseline.deployment`, `.hpa`, `.namespace`: `{path, sha256}` references to private JSON captures. The baseline must be revision 233, unpaused, replicas 2, HPA 2/2, maxSurge 1/maxUnavailable 0, minReadySeconds 30, grace 735s, preStop `/bin/sleep 30`, all three `CHRONO_AGENT_MANAGER_POD_*` downward-API variables, and `RUST_MIN_STACK=16777216`.
- `candidate`: `image` as the production cma-api repository plus an immutable digest, `source` as the exact commit, and `publication` as a bound JSON receipt with `status: PASS`, the same `image` and `source`, and `exact_config_and_layers_match: true`. Preserve the producer's original evidence when adapting it to this interface.
- `rehearsal.deployment`: the exact executed candidate Deployment capture, bound by file hash. `rehearsal.receipt` must bind `status: PASS`, the candidate image/source, canonical `template_sha256`, and `checks` with PASS for `two_replicas`, `image_roll`, `create_reply`, `drain_wake`, and `hpa_2_3_2`. The packet cannot promote a prepared renderer into executed evidence. These checks are packet admissions, not a claim that all customer features or clean shutdown were proven.
- `rehearsal.exceptions`: exact leaf differences with `path`, `production_present`, `rehearsal_present`, `production`, `rehearsal`, and `reason`. All differences, including local resource/reference substitutions, require exact review through the packet hash. No implicit field removals or wildcards. Only placement metadata, service-account names, local config/secret reference names, and CPU/memory resource fields can be listed. Identity, stack, env values, readiness, lifecycle, grace, commands and image differences cannot be excepted. The executed rehearsal must use the same immutable image reference.

The Python process holds a nonblocking BSD flock on the existing canonical writer-lock inode. macOS `lockf(1)` uses the same locking primitive. `LOCK_HELD` is ignored; there is no recursive shell invocation or `--locked` escape. Contending launchers fail before policy consumption. The inode remains after release. Patch and evidence files are regular files with private permissions; no shell or process-substitution path is involved.

Under the lock, execution reads the Deployment/HPA/namespace/pods/EndpointSlices, checks exact baseline and serving process identity, and server-dry-runs a JSON patch. The sole replace operation targets the named `api` container image. UID, resourceVersion, generation, revision, and whole-spec tests reject a changed preimage. The dry-run spec must equal the baseline with that one image replaced. A durable no-retry intent is written before policy/dispatch. The launcher calls `team.sh policy-check deploy` itself immediately before the mutation, checks the selected route, rechecks the write window in-process, then makes exactly one patch request. It never accepts a cached policy receipt. A denied/failed check cannot dispatch. Transport uncertainty remains `UNKNOWN_NO_REPLAY`; changing the output directory cannot replay the same sealed packet. Inspect/read back before any newly authorized operation.

The launcher reports only `IMAGE_PATCH_ACK_READBACK`. This is not rollout completion. The packet's read-only observer and executor must retain old/new Pod UID/process/endpoint evidence through natural drain, require two serving Ready replacement pods without restarts, and complete the stated health watch. Any recovery is a separate reviewed operation with its own approval. No old binary downgrade or replica contraction is implicit in this tool.
