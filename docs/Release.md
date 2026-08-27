# Release

A release is a merge into `main`. There is no separate step before it and none after it: the merge decides the version, publishes the images and writes the record.

This document is the procedure. The rules it implements are in [CLAUDE.md](../CLAUDE.md) §6.

---

## 1. What a release produces

| Artifact | Where | Integrity contract |
|---|---|---|
| `ghcr.io/chronoaiproject/cma-api:vX.Y.Z` | GHCR, private | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-web:vX.Y.Z` | GHCR, private | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-runtime:vX.Y.Z` | GHCR, private | intended write-once; requires audited writer access and package history |
| the same three at `:<short-sha>` | GHCR, private | intended write-once under the same audited contract |
| three OCI transfer archives with JSON metadata | the `Publish release` Actions run, readable under repository artifact access | yes; retained for up to one day from upload |
| annotated tag `vX.Y.Z` | this repository | write-protected after creation by the required active ruleset |
| GitHub release `vX.Y.Z` | this repository | notes editable |

The release page carries three things:

1. **The release notes**, under a `## Release Notes vX.Y.Z` heading the publisher writes, followed by the three subsections exactly as the pull request declared them.
2. **The source archives** — `Source code (zip)` and `Source code (tar.gz)`. GitHub generates these from the tag and attaches them to every release, so nothing uploads them and there is no second copy to go stale.
3. **The images** — every reference in a block you can copy, and a link to each package in GHCR.

The OCI archives are not release assets, but they are an additional, ephemeral distribution surface for the private images. They are unencrypted, and anyone allowed to download this internal repository's Actions artifacts can receive them; that reader set is not assumed to be identical to the GHCR package ACL. One-day retention limits how long they are available, not who can read them. SDKs remain repository-only and untouched: the release workflow neither packages nor publishes anything under `sdk/`.

The workflow intends to assign each image tag once, and a retry may retain an
exact existing image tag, annotated tag or release record after validation.
GHCR offers this workflow no compare-and-swap for a tag PUT. Even with every
external writer excluded, a sequential rerun could receive stale "absent"
listings after an earlier ambiguous push, rebuild different bytes, and PUT the
same names again. The second absence check narrows that window but cannot close
it. The audited writer and package-history prerequisites below are therefore
required before treating a completed release tag as write-once. There is
deliberately no `latest`: a moving tag is something a manifest can be pointed
at by accident, and [CLAUDE.md](../CLAUDE.md) §5 forbids a session sitting on
an image whose contents can change underneath it.

The version lives in the tag and nowhere else. `api/Cargo.toml` and `web/package.json` stay where they are — a bot committing a bump to `main` would put it ahead of `develop` and buy nothing, since the number in those files is not what anything deploys.

---

## 2. Cutting a release

1. Open a pull request from `develop` into `main`. `main` accepts them from nowhere else.
2. Apply exactly **one** of `release:major`, `release:minor`, `release:patch`. This is the whole of the version decision — the number itself is computed from the highest existing tag.
3. Give the body a **`## Release Notes`** section. It is mandatory and its shape is exact:

   ```markdown
   Reviewer-facing summary goes up here, outside the section, and is not published.

   Closes #N

   ## Release Notes

   ### What's New
   - Publish versioned images to GHCR on merge to main

   ### What's Fixed
   - Nothing this release

   ### What's Changed
   - develop into main now requires a release label
   ```

4. Get the checks green and merge.

### What the format check rejects

Each of these fails the gate, with the offending line quoted back and the required shape printed:

| Rule | Rejected example |
|---|---|
| The section exists, exactly once | no `## Release Notes` heading, or two of them |
| All three subsections, in this order | `What's Fixed` placed before `What's New` |
| Spelled and capitalised exactly | `### What's new` |
| Plain apostrophe, not a typographic one | `### What’s New` |
| At least one bullet under each | a subsection left empty |
| Only bullets underneath | `We added things.` as a paragraph |
| No version number | `0.1.12` under the heading |

Bullets may be `-` or `*`, and may nest. The section ends at the next `##` heading, so anything meant for the reviewer goes outside it and is never published.

**Do not write a version number anywhere.** The label decides how far the version moves; the publisher computes the number and writes it in as `## Release Notes vX.Y.Z`. A number typed into the pull request would be a guess at what the merge decides, so it is rejected rather than trusted.

The `Release declaration` check re-runs when a label or the body changes, so fixing either clears it without pushing a commit. `.github/release-notes.sh` is what reads the section, and the publisher runs the same script on the same body after the merge, so what ships is what was reviewed.

### Required checks on `main`

Branch protection on `main` requires these contexts, and `enforce_admins` is on — a required check that nothing reports leaves the pull request unmergeable by anyone, with no override:

```
Repository structure   Secret scanning   Backend   Frontend   Manifests
Images (api)           Images (web)      Source branch
Release declaration    Commit hygiene
```

A context is the job's `name:`, so **renaming a job is a branch-protection change.** Rename one without updating the list and every later pull request into `main` waits forever on a check that no longer exists. Update the two together, in the same rollout:

```sh
REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
gh api -X PATCH "repos/$REPOSITORY/branches/main/protection/required_status_checks" \
  -F strict=true -f 'contexts[]=Release declaration' ...
```

`Images (runtime)` is deliberately absent, matching `develop`. `ci.yml` builds it and the publisher will not release without it, so it is covered — but it is not a required check, and adding it is its own change.

### Required release write fences

These are hard prerequisites for merging a release, not recovery advice to
apply after publishing starts:

1. **GHCR has one audited package writer.** After the repository rename, audit
   the package roles, inherited organisation access, Actions repository access
   and every workflow permission for `cma-api`, `cma-web` and `cma-runtime`.
   The only identity able to write these three packages during ordinary
   operation must be this repository's `Publish release` image jobs. Remove
   human, team, token, other-repository and other-workflow write paths. Record
   the package-settings or API evidence with the release-control change. The
   current ACL check returned `403`, so this prerequisite is **not yet proven**.
2. **`v*` tags reject update and deletion.** After the repository rename and
   before the first release merge, create a repository ruleset targeting
   `refs/tags/v*`, with enforcement `active`, exactly the `update` and
   `deletion` rules, no `creation` rule, and no bypass actor. A creation rule
   would block the publisher from creating a new release tag. Do not infer
   success from the create response; GET the installed ruleset and verify its
   schema:

   ```bash
   set -euo pipefail
   REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
   RULESET_ID='<active-v-tag-ruleset-id>'
   gh api "repos/$REPOSITORY/rulesets/$RULESET_ID" > ruleset.json
   jq -e '
     .target == "tag" and .enforcement == "active" and
     .conditions.ref_name.include == ["refs/tags/v*"] and
     (.conditions.ref_name.exclude // []) == [] and
     ([.rules[].type] | sort) == ["deletion", "update"] and
     (.bypass_actors // []) == []
   ' ruleset.json
   rm ruleset.json
   ```

   The current repository has no tag ruleset, so this prerequisite is also
   **not yet satisfied**. Never weaken or bypass it to make a failed release
   retry succeed.

Image reconciliation is authorized to reuse an existing release tag only
under the exclusive-writer contract and only when package history ties that
exact digest to an earlier attempt of this publisher. Exact version/revision
labels and platform shape can be forged; they are not provenance for a tag
that was present before the writer audit. If provenance is missing, stop and
treat the tag as an integrity incident rather than deleting, overwriting or
adopting it.

Then, on merge, `publish.yml`:

- resolves the pull request the merge commit came from, re-reads its label, and re-extracts its `## Release Notes` section — checked again rather than trusted, because an administrator can merge past a failing check;
- computes the next version from the numerically highest canonical `vX.Y.Z`
  tag, seeding `v0.0.0` when there is none; on a whole-workflow rerun, reuses
  the single canonical tag peeled to this commit only after its declared bump,
  annotated target and exact assembled message all match;
- reconciles each image's version and short-SHA tags before any build: two
  absent tags authorize a build, two equal and valid tags reuse the published
  digest, and one valid tag repairs only its missing alias from that digest;
- fails without building or rewriting anything if tag discovery fails, the two
  tags disagree, or an existing image has the wrong labels or platform shape;
- builds, scans and pushes an OCI-media image only when reconciliation proved
  that both release tags are absent;
- lists the two tags again in a read-only check immediately before publish and
  fails if either appeared; this narrows the build-and-scan race window but is
  not a registry compare-and-swap, so it does not replace exclusive write
  access;
- after all three images are in GHCR, exports each published digest to a one-day OCI transfer artifact without rebuilding it;
- immediately before creating the tag, requires exactly one non-empty,
  unexpired transfer artifact for each image in the current workflow run;
- creates the annotated tag and GitHub release last, reusing either only when
  its target, annotation, title, body and published state are exactly what this
  run would create.

Watch it under **Actions → Publish release**. It takes roughly as long as the image job on the pull request did, because it restores the same layer cache.

Prefer **Re-run failed jobs** because it repeats only the work that needs to
converge. On **Re-run all jobs**, the planner, annotated tag and GitHub release
reconcile exact state and fail closed on disagreement. Each image leg also
reconciles before building, but GHCR's stale-read/CAS limitation still applies;
use a whole-workflow rerun only with the writer and history checks above. A
retried image job skips a visible equal pair, repairs one visible missing alias
from the existing digest, and builds only when both listings report absence.
This matters because the Dockerfiles refresh OS packages, so the same commit
can build different bytes on a later attempt. A retried export reads the
currently published digest and replaces only its same-named artifact from this
workflow run; the release preflight still requires exactly one artifact per
component before it can create the tag. A visible version, target, image digest,
annotation or release-record disagreement stops without rewriting it.

---

## 3. Fetching what was released

The release body lists the three GHCR references for direct consumers. When a deployment registry cannot authenticate to GHCR, use the OCI archives from the exact successful **Publish release** run. Each expires up to one day after its export upload, which can be earlier than the workflow's completion. Do not substitute `docker pull`, `docker tag`, `docker push`, or `docker save`: those paths do not prove that the destination kept the published root manifest and every platform manifest.

The transfer requires `gh`, `jq`, Docker, and OpenSSL. Run it from this repository's checkout, set the release version, and copy the numeric run ID from the successful run's Actions URL. The following checks that the run succeeded at the commit named by the release tag, derives the short revision from that commit, and downloads all three artifacts into fresh temporary directories:

```bash
set -euo pipefail

REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
RUN_ID='<successful-publish-run-id>'
VERSION=vX.Y.Z
TRANSFER_DIR="$(mktemp -d)"
AUTH_DIR="$(mktemp -d)"
trap 'rm -rf "$TRANSFER_DIR" "$AUTH_DIR"' EXIT HUP INT TERM

run_json="$(gh run view "$RUN_ID" --repo "$REPOSITORY" \
  --json workflowName,conclusion,headSha,url)"
jq -e '.workflowName == "Publish release" and .conclusion == "success"' \
  <<<"$run_json"
HEAD_SHA="$(jq -r '.headSha' <<<"$run_json")"
test "$HEAD_SHA" = "$(gh api "repos/$REPOSITORY/commits/$VERSION" --jq '.sha')"
REVISION="${HEAD_SHA:0:7}"

for component in api web runtime; do
  mkdir "$TRANSFER_DIR/$component"
  gh run download "$RUN_ID" --repo "$REPOSITORY" \
    --name "cma-${component}-${VERSION}-${REVISION}-oci" \
    --dir "$TRANSFER_DIR/$component"
done
```

Inspect and validate the JSON instead of sourcing it as shell code. This checks every selected identity, including the full and short commit revisions, the archive name and checksum, the source references, and the intended platform set. The exporter writes these files only after the version and revision tags have the same raw root-manifest digest, every source and archive platform config carries the exact version/full-revision labels, and the archive reproduces the root digest. `sourceDigest` is the GHCR digest observed and reproduced by the exporter at export time; accept it as the release identity only after the exclusive-writer and package-history checks above rule out a preseed or stale-read overwrite. Neither `archiveSha256` nor the checksum GitHub shows for its enclosing Actions artifact is an image digest.

```bash
for component in api web runtime; do
  base="cma-${component}-${VERSION}-${REVISION}"
  metadata="$TRANSFER_DIR/$component/$base.json"
  archive="$TRANSFER_DIR/$component/$base.oci.tar"
  source_repository="ghcr.io/chronoaiproject/cma-$component"
  if [ "$component" = web ]; then
    expected_platforms='["linux/amd64","linux/arm64"]'
  else
    expected_platforms='["linux/amd64"]'
  fi

  jq -e --arg component "$component" --arg version "$VERSION" \
    --arg revision "$REVISION" --arg full_revision "$HEAD_SHA" \
    --arg repository "$source_repository" \
    --arg archive "$base.oci.tar" --argjson platforms "$expected_platforms" '
      .schemaVersion == 1 and
      .component == $component and .version == $version and
      .revision == $revision and .fullRevision == $full_revision and
      .sourceRepository == $repository and
      .versionReference == ($repository + ":" + $version) and
      .revisionReference == ($repository + ":" + $revision) and
      (.sourceDigest | test("^sha256:[0-9a-f]{64}$")) and
      .sourceReference == ($repository + "@" + .sourceDigest) and
      .archive == $archive and .archiveReference == $version and
      (.archiveSha256 | test("^sha256:[0-9a-f]{64}$")) and
      .platforms == $platforms
    ' "$metadata"

  actual_archive_sha="sha256:$(openssl dgst -sha256 -r "$archive" | awk '{print $1}')"
  test "$actual_archive_sha" = "$(jq -r '.archiveSha256' "$metadata")"
  jq '{component, version, revision, fullRevision, sourceReference, sourceDigest, platforms}' \
    "$metadata"
done
```

Use the same immutable Skopeo image as the publisher. Authenticate it only to the destination registry; the source is the downloaded archive. The copy must keep both flags shown here. If the destination cannot preserve the media types or digest, `--preserve-digests` fails instead of silently rewriting the image.

```bash
SKOPEO_IMAGE='quay.io/skopeo/stable@sha256:4fcfc74eb89ec72dcd8caa7fbbf2a5f77d9fe3e2f5ac9e39192702f49c6d4d1a'
TARGET_REGISTRY=registry.example.com
TARGET_NAMESPACE=chrono
TARGET_USER=operator

skopeo_version="$(docker run --rm "$SKOPEO_IMAGE" --version)"
printf '%s\n' "$skopeo_version" \
  | grep -Eq '^skopeo version 1\.22\.2( commit: [0-9a-f]{7,40})?$'

read -rsp 'Target registry token: ' TARGET_TOKEN
printf '\n'
printf '%s' "$TARGET_TOKEN" | docker run --rm -i \
  --user "$(id -u):$(id -g)" --env HOME=/tmp \
  --volume "$AUTH_DIR:/auth:rw" \
  "$SKOPEO_IMAGE" login --authfile /auth/config.json \
  --username "$TARGET_USER" --password-stdin "$TARGET_REGISTRY"
unset TARGET_TOKEN

SKOPEO_WRAPPER="$TRANSFER_DIR/run-pinned-skopeo"
export SKOPEO_IMAGE
export SKOPEO_TRANSFER_DIR="$TRANSFER_DIR"
export SKOPEO_AUTH_DIR="$AUTH_DIR"
cat >"$SKOPEO_WRAPPER" <<'SKOPEO'
#!/usr/bin/env bash
set -euo pipefail
: "${SKOPEO_IMAGE:?}"
: "${SKOPEO_TRANSFER_DIR:?}"
: "${SKOPEO_AUTH_DIR:?}"
exec docker run --rm \
  --user "$(id -u):$(id -g)" --env HOME=/tmp \
  --volume "$SKOPEO_TRANSFER_DIR:/transfer:ro" \
  --volume "$SKOPEO_AUTH_DIR:/auth:ro" \
  --workdir /transfer \
  "$SKOPEO_IMAGE" "$@"
SKOPEO
chmod 0700 "$SKOPEO_WRAPPER"

SKOPEO_COMMAND="$SKOPEO_WRAPPER" \
SKOPEO_AUTHFILE=/auth/config.json \
  .github/import-release-images.sh \
    "$TRANSFER_DIR" "$VERSION" "$HEAD_SHA" \
    "$TARGET_REGISTRY/$TARGET_NAMESPACE"

rm -rf "$TRANSFER_DIR" "$AUTH_DIR"
trap - EXIT HUP INT TERM
```

The importer validates every metadata field, archive checksum, raw root digest,
platform config and exact version/full-revision label first. It then inspects
all six destination tags before the first copy: an equal tag is kept, any
mismatch aborts the whole import unchanged, and only an absent tag is copied
with `--all --preserve-digests` and verified afterward. This makes a retry
resume after a partial successful copy instead of overwriting it. Do not run
another writer against those six tags between preflight and copy.

Digest equality proves that `web` retained its amd64+arm64 index and that `api` and `runtime` retained their exact single linux/amd64 manifests. The final commands remove the downloaded archives, importer scratch data, wrapper, and temporary registry authentication; the trap does the same after an error. Copying images into a registry only makes them available. It does not edit `infra/`, deploy a workload, or adopt the new runtime image.

---

## 4. What a release deliberately does not do

**It does not deploy, and it does not touch `infra/`.** The manifests keep naming whatever image they already name. A release makes a version available; someone still has to decide to run it.

**It especially does not move `CHRONO_SANDBOX_IMAGE`.** Under [CLAUDE.md](../CLAUDE.md) §5 that value *is* the runtime version, and every session running a different one is rotated onto it — transcript snapshotted, sandbox replaced, agent resumed. Editing it is a rollout of every live session, which is why publishing a runtime image and adopting it are separate acts. Adopt one by editing `infra/api/configmap.yaml` in its own pull request, deliberately, when a rotation is what you want.

Changing an image reference in `infra/` is a deployment-surface change and belongs in the same pull request as anything else that changes with it (§4).

### Paired storage and runtime adoption

The release that adds durable skill-pack custody cannot be deployed as an API-only image change. It introduces two paired contracts:

- `chrono-bucket` must provide the durable mutation-generation fence before CMA can store a skill object. Drain every older storage-service replica and every direct writer sharing its S3 credentials before admitting the new CMA API. A delayed unconditional writer can otherwise finish after a generation-2 deletion acknowledgement and restore private bytes. Keep mutation tombstones outside every expiry, compaction, or lifecycle-deletion policy for the lifetime of the bucket.
- The runtime image from the same CMA release must be published, copied into the deployment registry, and deliberately adopted through `CHRONO_SANDBOX_IMAGE` before the new API image is deployed. The paired runtime accepts the exact four-MiB prelude the control plane validates; `v0.7.0` still has the framework's two-MiB request default and will return `413` for a prelude the new control plane correctly accepted.

Verify the storage deployment with a disposable logical key before starting CMA: commit generation 1 with its exact body SHA-256, commit deletion generation 2, confirm reads report the tombstone as absent, then prove a delayed or retried generation-1 write is rejected. Remove the disposable payload while retaining whatever tombstone the storage contract requires. Do not use a mixed old/new storage rollout as this probe: the drain is a prerequisite, not something one successful request can prove.

Publishing the matching runtime still does not rotate sessions by itself. After verifying its digest under the release writer/history fence, copy that version tag to the registry used by the cluster, update `CHRONO_SANDBOX_IMAGE` in the deliberate adoption pull request, and deploy that config with the new API. Never deploy the new API while the manifest still names the old runtime.

Apply and verify the runtime ConfigMap before allowing the new API Deployment to start. Do not apply `infra/api/` as an unordered directory: Deployment pods consume the ConfigMap through `envFrom`, and a pod that starts with the previous value will not reload a later ConfigMap update. The observed value must exactly equal the verified runtime reference copied into the deployment registry before the Deployment is applied:

```sh
kubectl --namespace chrono-bot apply -f infra/api/configmap.yaml
kubectl --namespace chrono-bot get configmap cma-api \
  -o jsonpath='{.data.CHRONO_SANDBOX_IMAGE}{"\n"}'
# Compare the printed value with the copied, digest-verified runtime reference.
kubectl --namespace chrono-bot apply -f infra/api/deployment.yaml
```

---

## 5. One-time MongoDB session namespace cutover

The release that introduces `sessions_v2` and `session_events_v2` cannot be rolled out alongside a CMA version that still mutates `sessions` and `session_events`. MongoDB validators can reject old inserts and replacements, but they do not fence deletes. `CHRONO_MONGODB_LEGACY_CUTOVER=drained` is therefore an operator assertion, not a condition CMA can discover or prove.

Run this once for a database whose legacy names are still writable collections:

1. Confirm the old deployment is healthy and record the current collection types for `sessions` and `session_events`.
2. Stop every CMA replica that can run the legacy writer. Wait for all pods/processes to terminate and for their outstanding MongoDB and object-storage operations to finish. Do not start the replacement while any old replica can still receive work.
3. At this quiescent boundary, take the deployment's normal transaction-consistent MongoDB backup or snapshot and a version-retained recovery point for the configured `CHRONO_CONTEXT_BUCKET`. Keep their timestamps and provenance as one recovery unit. If object storage is deliberately unconfigured, record that fact with the MongoDB backup. Do not resume any writer between the two captures.
4. Set `CHRONO_MONGODB_LEGACY_CUTOVER` to the exact value `drained`.
5. Start exactly one new CMA replica and let startup finish. Do not overlap this step with an old binary or start a second migration candidate.
6. Verify directly in MongoDB that `sessions` is a read-only view of `sessions_v2` and `session_events` is a read-only view of `session_events_v2`, then verify that the replacement's `/ready` endpoint returns `200`.
7. Replace the verified migration replica sequentially with one unflagged replica. The deployment is deliberately single-replica with a `Recreate` strategy, so never overlap the flagged and unflagged pods. Clear the live ConfigMap value, scale the deployment to zero, wait for the flagged pod to be deleted, and only then scale back to one:

   ```sh
   kubectl --namespace chrono-bot patch configmap cma-api --type merge \
     -p '{"data":{"CHRONO_MONGODB_LEGACY_CUTOVER":""}}'
   kubectl --namespace chrono-bot scale deployment/cma-api --replicas=0
   kubectl --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-api --timeout=120s
   kubectl --namespace chrono-bot scale deployment/cma-api --replicas=1
   kubectl --namespace chrono-bot rollout status deployment/cma-api --timeout=5m
   ```

   Verify that exactly one API pod exists, the live ConfigMap value is empty, that sole replacement pod was created after the value was cleared, and its `/ready` endpoint returns `200`. Do not resume ordinary traffic until all four checks pass.

Default startup refuses a writable or unproven partial legacy state before renaming, dropping, or otherwise mutating either legacy namespace. Before the first action on a genuinely empty database, or after the explicit drain acknowledgement, CMA records durable in-progress provenance so a crash can resume that exact migration. Once both views are installed, it majority-writes completion with the authoritative collection UUIDs. Later startup refuses a missing or replacement namespace instead of treating post-completion data loss as a fresh database. A generic partial state without that provenance is not inferred to be safe.

---

## 6. One-time MongoDB coordination and session-writer activation

The release that makes runtime authority and cleanup debt durable moves the writable session collection from `sessions_v2` to `sessions_v3`. It leaves `session_events_v2` in place. The release that makes explicit skill grants and skill-pack custody durable also advances the `sessions_v3` document writer from `_cma_write_protocol: 1` to `_cma_write_protocol: 2`; this prevents an older whole-document replacement from silently removing `granted_skills` or `skill_packs`.

Neither transition may overlap a CMA process that predates it. A pre-coordination process can make an external provider call before its database write, and MongoDB validators cannot recall that call or fence a standalone delete. A coordination-protocol-3 process with session write protocol 1 can still replace a session with a document that does not know the new fields. `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` is the operator's assertion that database work and provider work from every such process have permanently stopped.

Run this procedure for every existing CMA database after the namespace cutover in section 5 is complete. A database receiving coordination protocol 3 and session write protocol 2 in the same release needs one drain. A database that completed coordination protocol 3 under an earlier build repeats the drain only for its one-time session-writer upgrade:

1. Confirm the old deployment is healthy, then stop every CMA binary that predates coordination protocol 3 or writes `sessions_v3` with `_cma_write_protocol: 1`. Wait for every pod/process, outstanding MongoDB operation, object-storage mutation, and external provider create, delete, or replacement operation to finish. Prevent the old workload from returning. Rotate or revoke its database, storage, and provider credentials where the deployment permits that stronger fence.
2. At this quiescent boundary, take one transaction-consistent backup or snapshot containing `sessions_v3` if it already exists, otherwise `sessions_v2`; `session_events_v2`; `session_coordination_receipts`; `_cma_session_namespace_cutovers`; `session_writes`; and `session_tombstones`. Capture a version-retained recovery point for the configured `CHRONO_CONTEXT_BUCKET` at the same boundary. Keep the MongoDB collections, object versions, and provenance records as one recovery unit. If object storage is deliberately unconfigured, record that fact with the MongoDB backup. Do not combine session data from one point in time with receipts, markers, skill packs, contexts, or handover objects from another.
3. Set `CHRONO_MONGODB_COORDINATION_CUTOVER` to the exact value `drained`. If section 5 has not yet run, set its separate legacy acknowledgement only after satisfying that section's drain conditions too.
4. Start exactly one new CMA replica. Do not start another activation candidate and do not restore any old replica while startup is in progress. A coordination activation can resume without the flag after its durable in-progress marker is recorded. An already coordinated session-writer upgrade can resume without the flag after the strict protocol-2 session validator is installed. If startup fails before either applicable fence is durable, re-establish the drain before reasserting the flag.
5. Verify directly in MongoDB that `sessions` is a read-only view of `sessions_v2`, `sessions_v2` is a read-only view of `sessions_v3`, and `sessions_v3` is the sole writable session collection. Confirm that its strict validator admits only `_cma_write_protocol: 2` with `_cma_coordination_protocol: 3`, and that every session document carries both exact values. Confirm that coordination receipts still carry `_cma_write_protocol: 1` with `_cma_coordination_protocol: 2`. Confirm that the coordination activation and receipt provenance records in `_cma_session_namespace_cutovers` are complete and UUID-bound, then confirm `/ready` returns `200`.
6. Replace the verified migration replica sequentially with one unflagged replica. The deployment is deliberately single-replica with a `Recreate` strategy, so never overlap the flagged and unflagged pods. Clear the live ConfigMap value, scale the deployment to zero, wait for the flagged pod to be deleted, and only then scale back to one:

   ```sh
   kubectl --namespace chrono-bot patch configmap cma-api --type merge \
     -p '{"data":{"CHRONO_MONGODB_COORDINATION_CUTOVER":""}}'
   kubectl --namespace chrono-bot scale deployment/cma-api --replicas=0
   kubectl --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-api --timeout=120s
   kubectl --namespace chrono-bot scale deployment/cma-api --replicas=1
   kubectl --namespace chrono-bot rollout status deployment/cma-api --timeout=5m
   ```

   Verify that exactly one API pod exists, the live ConfigMap value is empty, that sole replacement pod was created after the value was cleared, and its `/ready` endpoint returns `200`. Do not resume ordinary traffic until all four checks pass.

A genuinely empty database activates automatically. Any database that existed before startup requires the exact acknowledgement, even when its session collections happen to be empty. The activation majority-writes intent before fencing and renaming `sessions_v2`; the rename preserves its collection UUID. It then creates `sessions_v2` as a view of `sessions_v3`, establishes and audits the receipt collection, and completes a marker bound to both protected UUIDs. A completed marker authorizes verification only: startup will not recreate a missing session or receipt collection, repair a replacement collection, or infer a new empty store.

An existing database that already has a complete coordination-protocol-3 marker and the exact previous session contract (`_cma_write_protocol: 1`, `_cma_coordination_protocol: 3`, strict validation, and error action) uses the same drain acknowledgement for its one-time session-writer upgrade. Before mutating it, startup verifies the activation and receipt provenance UUIDs, namespace chain, indexes, and every non-session collection contract. It refuses the previous session contract without `drained`. With the acknowledgement, it installs the strict protocol-2 session validator first, then updates only `_cma_write_protocol` on documents whose coordination protocol is already 3 and whose revision is already a 64-bit integer. The migration does not replace session documents, so `granted_skills`, `skill_packs`, and every other session-owned field are preserved.

The strict session validator is also the durable resume boundary. The exact previous validator means the old writer is not yet fenced and a fresh drain acknowledgement is required. The exact current validator with any remaining protocol-1 documents means the old writer is fenced and startup may finish the document migration without the flag. Any other validator or validation mode, including a validator that admits both protocols, a missing or extra clause, a non-strict level, a warning action, or a collection UUID mismatch, is refused without mutation even when `drained` is set.

### Unreleased coordination protocols 1 and 2

Coordination protocols 1 and 2 were experimental formats that were never released to production. Their authority handovers could write `rotating`, continue accepting cumulative usage from the old runtime, and later switch authority without recording the exact event position of that switch. Once such a history exists, neither the activation marker nor a protocol-1 receipt can reconstruct the missing boundary. Treating `rotating` as that boundary can overstate usage.

Protocol 3 therefore refuses every protocol-1 or protocol-2 activation phase, including `in-progress`, `complete`, and the experimental protocol-2 `upgrade-in-progress` phase. It refuses them whether or not `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` is set: the acknowledgement can prove that old writers stopped, but it cannot manufacture a historical accounting boundary. Protocol-3 `upgrade-in-progress` markers produced by the unshipped upgrade path are also refused because their shape does not prove which source history they came from. Startup performs no marker replacement, validator change, or document migration for any of these states.

The supported protocol-3 start states are a fresh or pre-coordination database entering initial activation, a protocol-3 `in-progress` marker resuming that initial activation, and a protocol-3 `complete` marker verifying an already activated database. Markerless activation requires the exact pre-coordination source shape: `sessions_v2` is still a collection, `sessions_v3` is absent, and no coordination-receipt collection or provenance exists. A coordinated namespace whose activation marker was lost is refused before replacement provenance can be inserted. `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` remains the required acknowledgement for first activation of an existing pre-coordination database and for the exact session-write-protocol-1 to protocol-2 transition described above. It is not a coordination-marker upgrade switch and cannot replace lost proof.

Current session documents use write protocol 2 and coordination protocol 3. Coordination receipts keep write protocol 1 and use coordination protocol 2 only. Every completed authority-switch receipt must name a positive 64-bit `runtime_started_seq`, and startup verifies that event marker. A coordination-protocol-1 receipt, a receipt validator that still admits it, an older session coordination protocol, or any unknown or downgraded validator makes even a protocol-3 `complete` store fail closed. The narrowly authorized session writer transition does not rewrite receipts or repair any other completed contract.

For an experimental database carrying any refused marker, preserve a transaction-consistent backup for diagnosis, then reset the whole experimental database or restore a whole transaction-consistent backup from before coordination was activated. Do not delete or synthesize an individual marker, rewrite receipts, or relabel validators to force startup. After restoring a genuine pre-coordination state, follow the initial activation procedure above and satisfy its offline drain before setting `CHRONO_MONGODB_COORDINATION_CUTOVER=drained`.

Rollback to coordination v1 or v2 is intentionally unsupported. An older startup rejects the protocol-3 marker, while an already-running old coordination writer is rejected by the strict coordination-protocol-3 contract.

Rollback to any image whose `sessions_v3` writer predates write protocol 2 is also unsupported after the protocol-2 validator is installed. Its startup rejects the changed session contract, and an already-running predecessor's protocol-1 insert, update, raw-append, coordination, or whole-document replacement fails document validation rather than erasing the new session fields. MongoDB still cannot fence a standalone delete or recall a provider call, which is why the offline drain is mandatory before installing the fence. Never downgrade a validator or marker to make an old process start.

Rollback to a pre-coordination image is intentionally unsupported. Such an image expects `sessions_v2` to be a collection; after activation it is a view, so startup and direct old-style insert, replace, and delete operations must fail. Keep old workloads disabled after activation rather than treating that refusal as a reason to restore one.

If protected data or proof is lost, never create a new `session_coordination_receipts` collection and never synthesize or delete an activation marker to make startup pass. Restore `sessions_v3`, `session_events_v2`, `session_coordination_receipts`, all provenance records in `_cma_session_namespace_cutovers`, `session_writes`, `session_tombstones`, and the version-retained `CHRONO_CONTEXT_BUCKET` recovery point as one unit. Restoring MongoDB alone can resurrect a `Stored` custody row after its exact object was deleted; restoring object storage alone can expose bytes whose cleanup acknowledgement remains durable. Simultaneous loss of protected data and its durable proof is outside automatic recovery and requires an explicit incident recovery decision.

---

## 7. When it goes wrong

**The publish run failed in an image job.** Choose **Re-run failed jobs**. Before
doing any build work, the failed leg lists both release tags. If neither exists,
it builds again. If both exist at the same valid digest, it skips the build. If
only one exists, it validates that image and creates only the missing alias from
its digest. A lookup failure, invalid image or disagreement fails closed. This
normally makes a retry converge when a registry push succeeded but the action
did not report success. A stale tag listing can still miss that push; if package
history shows either name moved, stop rather than accepting the rebuilt digest.

**It failed after the images pushed but before the tag.** Prefer **Re-run failed
jobs** so only the failed export or release work resumes. Under the verified
exclusive-writer and package-history fence, those jobs re-read and export the
current tagged digests. A whole-workflow rerun is more expensive and retains an
existing digest only when reconciliation can see it; stale absence responses
remain a residual risk, so review package history before accepting the release.
An export retry may replace only its same-named artifact in this workflow run.
Immediately before the tag step, the release job requires exactly one non-empty,
unexpired transfer artifact for each component. Never rewrite either image tag
by hand.

**It failed after the tag was pushed but before the GitHub release completed.**
Choose **Re-run failed jobs**. The tag reconciler reuses the annotated tag only
if it targets this workflow's commit and its message is byte-for-byte equal to
the assembled release notes. The release reconciler likewise reuses an existing
release only if its tag, title, body, draft state and prerelease state are exact;
otherwise it stops without editing the existing record. The artifact preflight
still runs first, so an expired transfer artifact is never hidden by a retry.

**A commit reached `main` outside a pull request.** The run fails deliberately:
there is no label, so there is no declared bump, and guessing one would put a
number on a release nobody sized. Revert it and merge it properly. There is no
ordinary hand-publishing path because a standing second package writer would
invalidate the release integrity contract.

If an operational incident makes a manual release unavoidable, treat it as a
quiesced break-glass change: cancel and drain `Publish release`, prove no
package or tag writer is active, record fresh GHCR writer-ACL evidence and the
GET-verified tag ruleset payload, peer-review the exact commit, version, notes,
six image references and three digests, and grant one time-bounded writer only
for that change. Remove the grant and re-audit both fences before any workflow
is resumed. Never delete or move an existing release tag as part of recovery.

**The release pull request reads `BEHIND` and will not merge.** `develop` is missing the merge commit the *previous* release left on `main`. It carries no content — `git diff develop..main` is empty — but a merge commit is still a commit, and a protected branch requiring up-to-date branches will not take a pull request without it.

Since `v0.4.0` the publish workflow pushes a release-specific branch and opens that back-merge itself, as its last step, so this should not recur. If repository policy refuses Actions permission to open pull requests, the job fails and leaves the branch in place rather than reporting a false success. Open the branch against `develop` manually, then rerun the failed job; it recognizes the open pull request and completes without creating a duplicate. If the release gate later says `develop contains main` failed, merge that back-merge pull request. Merging `origin/main` into `develop` by hand does the same thing; do not force push, `develop` is protected.

A back-merge that **conflicts** is left open deliberately rather than resolved automatically: a conflict means `develop` moved during the release, and a resolution nobody chose is how a release silently reverts something.

**The push to GHCR is refused.** Two causes, in order of likelihood. The repository's default workflow permission is read-only — the explicit `permissions:` blocks should cover it, but confirm under Settings → Actions → General. Failing that, the organisation restricts package creation by Actions, which is not something this repository can change.

**Two merges landed together.** They do not race with each other; the workflow
serializes on a single concurrency group, and the second computes its version
after the first has tagged. This serialization does not cover an external
package writer, which is why the exclusive-writer prerequisite is mandatory.

**Three or more landed inside one build window.** A concurrency group holds one *pending* run, not a queue, so a third merge supersedes the second's pending run and that merge is marked cancelled having published nothing.

The code is not lost: the run that does publish is building a later commit on `main`, which contains it. What is lost is that merge's notes and its declared bump — so a `release:major` can be absorbed into a following `release:patch`, and the version that ships understates what is in it.

Re-run the cancelled run from the Actions tab before the next one tags and it publishes normally. After that, its number is no longer the next one, so fold its notes into the release that did go out instead. Rare by construction, since every merge into `main` is a release — but the only sign is a cancelled run rather than a failed one, which is easy to read as noise.
