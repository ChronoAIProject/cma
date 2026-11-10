# Owner-key identifier Secret cutover

This is the train handoff for #1588, coordinated with parity issue #1587 / PR
#1589. Only **cma-web-hotfix**, in the approved release execution context, runs
production commands. Merge/review does not authorize this cutover. The target is
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
set +x
umask 077
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
  "secret": {"uid": "<from the public fence>", "resourceVersion": "<from the public fence>"},
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

## Native typing drift and parity handoff

The recorded flag is `CMAEG_NATIVE_TYPING_ENABLED`, not a new WhatsApp intake
activation. It controls Discord typing and private WhatsApp typing plus a read
receipt. The **CMAEG feature/release owner**, **Discord integration/product owner**,
and **WhatsApp integration/product/connection owner** must confirm intended
activation and recipient/read-receipt behavior. The lead routes that decision;
this repository has no CODEOWNERS mapping that identifies those people. NyxID's
catalogue owner is needed only for an independent identity/introspection change,
which this migration does not request. Keep the existing guarded repository flag
and tests unchanged pending that decision.

#1588 remains open until the train cutover/verification and activation decision
are recorded. If this change lands before #1589, merge develop into the parity
branch, retain these Secret references, remove its obsolete empty-placeholder
apply guard and update the parity description with actual migration status. Only
use cma-web-hotfix's final API handover for the image, generation, provenance and
`TOKIO_WORKER_THREADS` downward-API stanza; a prior rollback report is insufficient.
