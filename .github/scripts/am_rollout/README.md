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

Under the lock, execution reads the Deployment/HPA/namespace/pods/EndpointSlices, checks exact baseline and serving process identity, and server-dry-runs a JSON patch. The sole replace operation targets the named `api` container image. UID, resourceVersion, generation, revision, and whole-spec tests reject a changed preimage. The dry-run spec must equal the baseline with that one image replaced. A durable no-retry intent is written before policy/dispatch. The launcher hashes and parses the same captured evidence bytes. It calls `team.sh policy-check deploy` itself immediately before the mutation, checks the selected route, reserves 25 seconds before both window end and the SGT blackout for its 20-second request bound, and checks patch-file integrity in-process. It then sends the already captured patch bytes as a literal subprocess argument, with no shell or path reopen. Dry-run uses the same captured bytes. The absolute mutation deadline caps both kubectl request duration and the subprocess timeout; terminal journals retain the original intent and patch hash. A client timeout cannot prove that the API server did not commit. It never accepts a cached policy receipt. A denied/failed check cannot dispatch. Transport uncertainty remains `UNKNOWN_NO_REPLAY`; changing the output directory cannot replay the same sealed packet. Inspect/read back before any newly authorized operation.

The launcher reports only `IMAGE_PATCH_ACK_READBACK`. This is not rollout completion. The packet's read-only observer and executor must retain old/new Pod UID/process/endpoint evidence through natural drain, require two serving Ready replacement pods without restarts, and complete the stated health watch. Any recovery is a separate reviewed operation with its own approval. No old binary downgrade or replica contraction is implicit in this tool.

## Optional local credential fixture

The production `opensandbox-api-key` volume uses GKE Secrets Store CSI. A local application rehearsal may use the single `local_opensandbox_secret_v1` rule in `controller.FIXTURE_RULE`. Set `rehearsal.credential_fixture` to `{ "rule": <exact FIXTURE_RULE>, "evidence": { "path": "...", "sha256": "..." } }`. This is a separate typed rule; it does not expand the ordinary exception paths. A prepared packet may leave evidence null, but cannot pass runtime admission or execute.

The rule pairs removal of the exact CSI source at `/spec/volumes/3/csi` with addition of exactly this source at `/spec/volumes/3/secret`:

```json
{
  "secretName": "cma-rev234-synthetic-opensandbox-key",
  "defaultMode": 288,
  "items": [{"key": "opensandbox-api-key", "path": "opensandbox-api-key"}]
}
```

The volume stays named `opensandbox-api-key`; the sole mount stays `/var/secrets`, readOnly, without subPath. Mode 288 is octal 0440. Production CSI driver, provider class and readOnly source must match the rule. The source changes must appear together; ConfigMap, hostPath, projected, optional, additional keys, another volume/index, helper mounts, or numeric substitutes for JSON booleans fail. UID/GID/fsGroup 10001 and container restrictions remain required. Env, readiness, lifecycle, grace and all other behavior remain subject to full-template parity. The real dry-run and mutation retain the original CSI source and replace only the image.

The credential evidence schema is `am-local-credential-fixture-v1`. It records observations from an actual isolated run. A proposal or a helper HTTP call does not qualify. `candidate.config_digest` and the publication receipt's `config_digest` must bind the same image configuration digest. Both the ordinary runtime receipt and credential evidence bind the following identities:

- Credential evidence: `status: PASS`, `source`, `image`, `config_digest`, canonical `template_sha256`, and `run_id`. The ordinary receipt supplies the matching `run_id`, image/source/template and all five existing rollout checks.
- `cluster` in credential evidence equals `local_cluster` in the ordinary receipt: `context` beginning `kind-`, loopback HTTPS `server` with port, and actual `kube_system_uid`. `namespace` equals the ordinary receipt's `local_namespace`, containing actual `name` and `uid`. It must match the executed Deployment's local namespace and differ from production.
- `secret` contains exactly `name`, `namespace`, actual `uid`, `resource_version`, `type: Opaque`, `immutable: true`, `data_keys: ["opensandbox-api-key"]`, `provenance: generated_for_this_local_run`, and `content_sha256`. The ordinary receipt's `fixture_secret` repeats name/namespace/UID/resourceVersion/digest. Never put the value, encoded value or an authorization header in either receipt.
- `fake_provider` is identical in both receipts: local namespace UID, actual `service_uid`, `origin` at a Service host ending in `.<local-namespace>.svc`, `isolated: true`, and `production: false`. URL credentials, external hosts and production namespace references fail.
- The ordinary receipt's `candidate_processes` lists every distinct candidate process encountered during the pair, roll and scale cases, with `pod_uid`, `container_id` and `image_id`. At least three candidate pods and the final pair in `final_pod_uids` are required. Credential `processes` must cover exactly those identities; retain all processes, including scale-down/drained pods, rather than reporting only the final pair.
- Each credential process records namespace UID, Secret UID/resourceVersion, actual `uid`, `gid`, `groups`, and `file`: `path`, resolved integer `resolved_mode` 288, `resolved_uid` 0 or 10001, `resolved_gid` 10001, content `sha256`, and true observations for `readable`, `valid_nonempty_header`, `read_only_mount`, `write_refused`, `non_destructive_write_check`. Inspect the resolved file and test refusal without truncating or modifying it. `effective_config` supplies `api_key_file`, `sandbox_provider`, boolean `retain_opensandbox`, and `sandbox_base_url`; it must select the actual startup file-reading branch and the isolated fake.
- `authentication.am_requests` contains actual AM request receipts with unique `request_id`, candidate `pod_uid`, `caller: actual_am`, `credential_source: startup_file`, successful numeric HTTP `status`, and `fake_service_uid`. `authentication.negative_checks` supplies distinct missing/wrong credential requests to that same fake, each with unique request ID and status 401/403. Negative request IDs cannot reuse successful request IDs. Helper-only success or an accept-all fake fails.

`test_fixture.py` constructs a complete synthetic receipt solely for offline regression tests. It is not runtime evidence. The runner must collect the identities and observations above and preserve their underlying captures for independent review. The controller checks consistency of supplied evidence; it cannot attest to the truth of a fabricated receipt.

This fixture qualifies the application's local key-file loading and authenticated fake-provider path. It provides no evidence of GKE CSI provisioning, SecretProviderClass resolution, IAM/Workload Identity, Secret Manager availability, rotation/distribution, remote image publication, or production approval. Those limits stay in the rollout packet. The exact old v2 tooling review applies only to its original bytes; fixture-capable tooling and every final executable packet require their own exact review.
