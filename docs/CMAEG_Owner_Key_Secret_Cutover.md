# Owner-key identifier Secret cutover

This is the train handoff for #1588, coordinated with parity issue #1587 / PR
#1589. **cma-web-hotfix coordinates the train; cmaeg-release executes the CMAEG step**
under the coordinator's active bridge lock, in the approved execution context.
Record any executor substitution before the first write. Merge/review does not
authorize this cutover; the train GO and current execution gates are still required. The target is
the first eligible evening train on 2026-11-11, with code merged by 14:00 SGT and
at least three hours before the train. The last fallback is the 2026-11-13 morning
train, with production verification complete by 15:00 SGT. A missed review/train
slot must be escalated to the lead; do not compress observation to meet a date.
The remaining merge windows are 11-11 14:00, 11-12 06:00 and 14:00, and
11-13 06:00 SGT, for the 11-11 evening, 11-12 morning/evening and final
11-13 morning trains respectively. Each merge must precede its train by at
least three hours.

## Contract and scope

The dedicated Opaque Secret `chrono-bot/cma-event-gateway-owner-keys` supplies
`CMAEG_OWNER_KEY_SERVICE_ID` and `CMAEG_OWNER_KEY_CUTOVER_ID` through explicit
`secretKeyRef` environment entries on container `cmaeg`. The ConfigMap contains
neither key, including empty placeholders. `infra/cmaeg/secret.yaml` declares
only the resource and key names, with no values.

The references are `optional: true` solely to preserve deployments where owner
keys are disabled. Existing startup validation requires tracking plus both
canonical, non-nil identifiers when `CMAEG_OWNER_KEYS_ENABLED=true`; a missing
Secret/key, empty value or invalid identifier then fails startup. Explicit env
entries override legacy ConfigMap `envFrom` during overlap. Settings debug output
reports presence only. This changes storage, not the service identity, attestation,
permission policy, key validity or worker-credential custody.

**Do not apply the full repository Deployment for this migration.** Its image pin
may lag production. The reviewed operator helper changes only the two env entries,
then the two ConfigMap fields and any last-applied annotation containing them.
It never changes an image, native flag, runtime pin, API resource or database.
Do not invoke an attestation command, an old migrator, an old repository image,
credential rotation, a master-key resolver or a Pod exec/env dump.

**Secret-first release gate:** after this change lands, create and verify the
Secret before **any** CMAEG deployment/config application from develop. The
committed ConfigMap no longer supplies these keys; enabled mode fails closed
without valid identifiers. Do not rely on legacy fallback during rolling overlap.
The train coordinator must record the Secret prerequisite as VERIFIED before
releasing the CMAEG image/config write gate.

## Preconditions and evidence

1. Obtain the release owner's GO and exclusive bridge lock. Preserve STOP-marker
   semantics. Freeze concurrent Secret/ConfigMap/Deployment writers through the
   final readback. Kubernetes has no atomic transaction across these resources:
   the helper checks their fences before and after writes, and a race requires a
   hold and recapture, not an automatic retry. Keep the API observation window clear.
2. Use the train's approved Kubernetes context and access path. The helper needs
   GET on these three named objects, CREATE on the dedicated Secret if absent,
   and PATCH on the named Deployment/ConfigMap. It grants no RBAC. An existing
   Secret must have exactly the two matching keys; a mismatch is an owner decision,
   never an overwrite. Do not apply the empty Secret declaration over a live Secret.
3. Record the current verified CMAEG image, source/artifact, UID, generation,
   readiness, replicas and rollback compatibility boundary using the existing
   bounded train captures. The application image stays unchanged for this config
   step. Both owner-key and tracking flags must already be enabled. Do not combine
   an identifier rotation or feature activation with this migration.
4. Establish a read-only owner-key baseline using an **already authorized owner's
   existing service-key client**, against that owner's management GET/list route.
   Record only pass/fail and HTTP status. A browser OAuth success does not prove
   the owner-key lane. Do not build/borrow an owner helper, access another owner's
   session, issue a new key, inspect customer credentials or manufacture a channel.
   The owner/lead must relay that result if the train operator lacks an approved
   client. Local negative-auth tests cover missing, malformed and disabled inputs.
5. Create a private evidence directory under the train's `notes/` (0700), files
   0600, `umask 077`, shell tracing off and core dumps disabled (`ulimit -c 0`). Do not capture raw ConfigMaps, Secrets,
   env arrays, `kubectl diff`, payloads, subprocess stderr, hashes of identifier
   values or last-applied annotations. The helper emits only resource fences,
   image identity and fixed reasons. Existing plaintext history is not erased by
   this migration; normal cluster audit/backup custody remains with its owners.

The helper is `.github/scripts/cmaeg-owner-key-cutover.py`; its sibling model must
be present. Reads are exact-object requests with deadlines. Secret payloads pass
in memory directly to `kubectl` stdin, never command arguments, output or files.
The helper suppresses upstream errors; `*_failed_or_uncertain` means inspect the
bounded state before deciding whether a write happened. It never retries writes.

## Execute in separate fenced phases

The following variables name the approved context and private evidence directory,
not secret values. Populate them through the train's normal setup. Do not execute
these commands from an unapproved workstation or as part of CI.

```sh
set -euo pipefail
set +x
umask 077
ulimit -c 0
python3 .github/scripts/cmaeg-owner-key-cutover.py capture \
  --context "$train_context" > "$train_evidence/owner-keys-before.json"
python3 .github/scripts/cmaeg-owner-key-cutover.py seed \
  --context "$train_context" --fence "$train_evidence/owner-keys-before.json" \
  > "$train_evidence/owner-keys-seeded.json"
```

`seed` copies the two original live ConfigMap strings into the new Secret in
memory. CREATE is exclusive: an existence race fails. An existing exact match is
a no-op; mismatched, extra, missing or malformed keys are refused. It compares the
Secret and ConfigMap in memory, emitting no values. No Pod restarts at this phase.
A lost creation response is recoverable by recapture and the matching no-op path.

After `seed` succeeds, perform this bounded key-presence-only readback before
any Deployment change. The helper already compared the original and copied values
in memory and validated their shape. This independent projection records only
key presence/count and resource identity, without values or identifier hashes:

```sh
secret_presence_template='{"uid":{{printf "%q" .metadata.uid}},"resourceVersion":{{printf "%q" .metadata.resourceVersion}},"key_count":{{len .data}},"service_id_present":{{if index .data "CMAEG_OWNER_KEY_SERVICE_ID"}}true{{else}}false{{end}},"cutover_id_present":{{if index .data "CMAEG_OWNER_KEY_CUTOVER_ID"}}true{{else}}false{{end}}}'
kubectl --context "$train_context" --namespace chrono-bot --request-timeout=10s \
  get secret cma-event-gateway-owner-keys -o "go-template=$secret_presence_template" \
  2>/dev/null > "$train_evidence/owner-keys-presence.json"
python3 - "$train_evidence/owner-keys-seeded.json" \
  "$train_evidence/owner-keys-presence.json" <<'PY'
import json, sys
with open(sys.argv[1]) as stream:
    seeded = json.load(stream)["state"]["secret"]
with open(sys.argv[2]) as stream:
    observed = json.load(stream)
assert seeded is not None
assert observed["uid"] == seeded["uid"]
assert observed["resourceVersion"] == seeded["resourceVersion"]
assert observed["key_count"] == 2
assert observed["service_id_present"] is True
assert observed["cutover_id_present"] is True
print("Secret key presence and identity verified")
PY
```

A failed read, absent/extra key or changed Secret fence is a HOLD. Do not recapture
through a mismatch to force acceptance. The Secret prerequisite is VERIFIED only
after both the successful seed/equality check and this presence readback.

Immediately before switching, capture again and review the public fences:

```sh
python3 .github/scripts/cmaeg-owner-key-cutover.py capture \
  --context "$train_context" > "$train_evidence/owner-keys-switch-fence.json"
python3 .github/scripts/cmaeg-owner-key-cutover.py switch \
  --context "$train_context" --fence "$train_evidence/owner-keys-switch-fence.json" \
  > "$train_evidence/owner-keys-switched.json"
```

The exact JSON Patch tests Deployment UID, resourceVersion, generation, container
name and current image, then replaces **only** that container's env array with
the old entries plus the two Secret references. Unexpected owner env/flag overrides
or envFrom sources are refused. Secret and ConfigMap fences and equality are checked
before/after the change. A pre-existing exact reference is a no-op. The template
change itself initiates the rolling restart; **do not add a second rollout restart**.
Keep maxSurge 1 / maxUnavailable 0 and the existing drain/grace settings.

## Verify before removing plaintext fields

Use the train's existing rollout, readiness and bounded log/metric observations:

- New generation observed; desired, updated, ready and available replicas agree;
  predecessor Pods drained; no restart/OOM, startup owner-key/cutover failure or
  new restore/decode/authentication error pattern. Observe at least 15 minutes
  after steady readiness, excluding any gap or overlapping API work.
- The same authorized service-key client repeats its baseline read with expected
  200 and unchanged ownership/permissions. Record only aggregate results; do not
  save headers, tokens, subject IDs, channel IDs or response bodies. If this cannot
  be performed, retain the ConfigMap values and hold completion.
- Existing channel management reads and natural event/dispatch traffic remain
  normal. Report unexercised channels honestly. No test message is authorized by
  this runbook; use natural traffic or an independently owner-approved test channel.
- Secret UID/resourceVersion remains the seeded value; env references name its two
  exact keys. Readiness and unchanged bytes alone are not an owner-authorization
  smoke. No key or attestation was rotated.

The train operator records the successful result in a 0600 JSON receipt with only:

```json
{
  "deployment_uid": "<from the public fence>",
  "generation": 0,
  "secret": {
    "uid": "<from the public fence>",
    "resourceVersion": "<from the public fence>"
  },
  "owner_key_read_passed": true,
  "existing_channels_passed": true,
  "observation_passed": true,
  "verified_at": "<UTC RFC3339 verification time>"
}
```

Replace generation 0 and the fence/time placeholders with observed nonsecret
metadata. The helper requires a matching generation/Secret and a result no more
than 30 minutes old. Boolean assertions represent the operator's collected evidence;
creating this file is not a substitute for the checks.

```sh
python3 .github/scripts/cmaeg-owner-key-cutover.py capture \
  --context "$train_context" > "$train_evidence/owner-keys-cleanup-fence.json"
python3 .github/scripts/cmaeg-owner-key-cutover.py cleanup \
  --context "$train_context" --fence "$train_evidence/owner-keys-cleanup-fence.json" \
  --receipt "$train_evidence/owner-keys-verification.json" \
  > "$train_evidence/owner-keys-cleaned.json"
python3 .github/scripts/cmaeg-owner-key-cutover.py capture \
  --context "$train_context" > "$train_evidence/owner-keys-final-fence.json"
python3 .github/scripts/cmaeg-owner-key-cutover.py verify \
  --context "$train_context" --fence "$train_evidence/owner-keys-final-fence.json" \
  > "$train_evidence/owner-keys-final.json"
```

Cleanup tests ConfigMap UID/resourceVersion, removes only the two data fields and,
when present, its legacy last-applied annotation containing those key names. It
preserves all other configuration and does not restart Pods. Dependency changes,
missing readiness or mismatched verification refuse cleanup. Final captures retain
no values, value hashes or Secret-bearing payloads. Post the aggregate production
evidence to #1588 only after the lead relays verification.

## Abort, rollback and recovery

Before the env switch, aborting leaves the serving Deployment untouched. A newly
seeded but unused Secret can remain; do not delete/recreate it on an uncertain
write. During rollout, preserve the existing healthy Pod and both value sources if
authentication/readiness fails, and stop the train for diagnosis. Do not remove
ConfigMap fields without the verification receipt.

After switching, **preserve the Secret and its env references**. `rollout undo`,
reapplying an old manifest, unsetting identifiers or restoring values to the
ConfigMap is not an approved rollback. This is a same-image config migration; a
failure here is a hold and configuration forward fix, not an image downgrade.
For an independently batched image release, use its separately approved, fenced
image-only rollback if its data boundary permits it; keep these Secret references
and unchanged values in place. CMAEG's current forward-fix-only boundary still
applies. Never use the pre-GitHub repository pin or run an old migrator.

## Native typing decision and parity handoff

`CMAEG_NATIVE_TYPING_ENABLED` controls Discord typing and private WhatsApp typing
plus a read receipt; it is not a new WhatsApp intake activation. The owner decision
relayed on 2026-11-11 preserves production ON behavior. #1627 / #1624 deliberately
align the repository ConfigMap and manifest contract to `true`; the absent-value
application default remains `false`. See [native typing](CMAEG_Native_Typing.md)
for the existing custody/writer gates and provider-acceptance limits.

#1588 remains open until the Secret cutover and production verification are
recorded. The separately resolved typing decision is not a Secret-cutover gate.
#1589 integrates the Secret references and removes the obsolete empty-placeholder
apply guard. Its Train E API generation 177 and Web generation 209 pins and
provenance follow the verified release handover and fresh bounded readback,
including the `TOKIO_WORKER_THREADS` downward-API stanza. Future train changes
require another verified parity capture; a prior rollback report is insufficient.
