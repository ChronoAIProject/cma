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
| closed D-09 runtime-boundary evidence | the same Actions run, readable under repository artifact access | exact release commit and runtime digest; retained for 90 days |
| annotated tag `vX.Y.Z` | this repository | write-protected after creation by the required active ruleset |
| GitHub release `vX.Y.Z` | this repository | notes editable |

The release page carries three things:

1. **The release notes**, under a `## Release Notes vX.Y.Z` heading the publisher writes, followed by the three subsections exactly as the pull request declared them.
2. **The source archives** — `Source code (zip)` and `Source code (tar.gz)`. GitHub generates these from the tag and attaches them to every release, so nothing uploads them and there is no second copy to go stale.
3. **The images** — every reference in a block you can copy, and a link to each package in GHCR.

The OCI archives are not release assets, but they are an additional, ephemeral distribution surface for the private images. They are unencrypted, and anyone allowed to download this internal repository's Actions artifacts can receive them; that reader set is not assumed to be identical to the GHCR package ACL. One-day retention limits how long they are available, not who can read them. SDKs remain repository-only and untouched: the release workflow neither packages nor publishes anything under `sdk/`.

The D-09 record is a separate Actions artifact under the same repository access
boundary. It retains closed outcomes, image and build identity, artifact hashes,
synthetic identities, status classes, and counts for 90 days. Provider response
bodies and credential values are discarded rather than retained as evidence.

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
Images (api)           Images (web)      Images (runtime)
TypeScript SDK         Rust SDK          Source branch
Release declaration    Commit hygiene
```

A context is the job's `name:`, so **renaming a job is a branch-protection change.** Rename one without updating the list and every later pull request into `main` waits forever on a check that no longer exists. Update the two together, in the same rollout:

```sh
REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
gh api -X PATCH "repos/$REPOSITORY/branches/main/protection/required_status_checks" \
  -F strict=true -f 'contexts[]=Release declaration' ...
```

`Images (runtime)`, `TypeScript SDK`, and `Rust SDK` are required on both
protected branches. The SDK jobs prove the repository-only packages build and
work for source consumers; requiring them does not publish either SDK.

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

   The repository satisfies this prerequisite through active ruleset
   `21664901` (`Immutable v* release tags`), verified with this exact target,
   rule set and empty bypass list on 2026-08-28. Never weaken or bypass it to
   make a failed release retry succeed.

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
- consumes the exact runtime transfer artifact, validates its complete metadata
  and archive checksum, pulls its immutable `sourceReference`, and runs the D-09
  runtime network-boundary campaign against the full release commit;
- retains one closed D-09 result for 90 days after every outcome and requires a
  pass before GitHub Release creation;
- immediately before creating the tag, requires exactly one non-empty,
  unexpired transfer artifact for each image in the current workflow run;
- creates the annotated tag and GitHub release last, reusing either only when
  its target, annotation, title, body and published state are exactly what this
  run would create.

The GHCR version and short-SHA tags necessarily exist before D-09 because the
release-candidate digest cannot be tested until GHCR has accepted it. A failed
or inconclusive D-09 result blocks GitHub Release creation and remains attached
to the workflow run for review; it cannot prevent or retract those image tags
and must not be described as doing so.

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
workflow run. A retried D-09 gate consumes that exact runtime artifact and never
rebuilds or retags the image. The release preflight still requires exactly one
artifact per component before it can create the tag. A visible version, target,
image digest, annotation or release-record disagreement stops without rewriting
it. If the one-day runtime transfer artifact has expired, rerun the complete
workflow under the same writer and package-history fences rather than bypassing
the gate.

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
SKOPEO_IMAGE='quay.io/skopeo/stable@sha256:4a16d57b37617a04b3d643079a477a2848efe892dffcdf0ce56df4262b65f810'
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

- `chrono-bucket` must provide the durable mutation-generation fence before CMA can store a skill object. CMA sends the fence as query metadata because NyxID preserves the route query but does not promise to forward caller-defined `X-Chrono-*` headers: the generation-1 POST carries exact fields `key`, `contentType`, `mutationGeneration`, and `contentSha256`; the generation-2 DELETE carries `key` and `mutationGeneration`. `allowRecreate` is an optional storage query field, but CMA deliberately omits it because a session skill-pack key is never recreated after its tombstone. Drain every older storage-service replica and every direct writer sharing its S3 credentials before admitting the new CMA API. A delayed unconditional writer can otherwise finish after a generation-2 deletion acknowledgement and restore private bytes. Keep mutation tombstones outside every expiry, compaction, or lifecycle-deletion policy for the lifetime of the bucket.
- The runtime image from the same CMA release must be published, copied into the deployment registry, and deliberately adopted through `CHRONO_SANDBOX_IMAGE` before the new API image is deployed. The paired runtime accepts the exact four-MiB prelude the control plane validates; `v0.7.0` still has the framework's two-MiB request default and will return `413` for a prelude the new control plane correctly accepted.

Verify the storage deployment through the same NyxID path CMA uses, with a disposable logical key `K`, body `B`, and lowercase hexadecimal `D = SHA-256(B)`. Capture the request that arrives at storage and require these exact query shapes, after normal URL encoding, with no `X-Chrono-Mutation-Generation`, `X-Chrono-Content-SHA256`, or `X-Chrono-Allow-Recreate` header:

```text
POST   /api/buckets/{bucket}/objects?key=K&contentType=application%2Foctet-stream&mutationGeneration=1&contentSha256=D
DELETE /api/buckets/{bucket}/objects?key=K&mutationGeneration=2
```

The POST acknowledgement must name `K`, generation `1`, digest `D`, and `fenceVersion: 1`. The DELETE acknowledgement must return `deleted: true`, generation `2`, and `fenceVersion: 1`. Confirm a read reports the tombstone as `404 OBJECT_NOT_FOUND`, then replay the exact generation-1 POST and require `409 STALE_MUTATION_GENERATION`; another read must still return the typed `404`. Remove the disposable payload while retaining whatever tombstone the storage contract requires. Do not use a mixed old/new storage rollout as this probe: the drain is a prerequisite, not something one successful request can prove.

Publishing the matching runtime still does not rotate sessions by itself. After verifying its digest under the release writer/history fence, copy that version tag to the registry used by the cluster, update `CHRONO_SANDBOX_IMAGE` in the deliberate adoption pull request, and deploy that config with the new API. Never deploy the new API while the manifest still names the old runtime.

Apply and verify the runtime ConfigMap before allowing the new API Deployment to start. Do not apply `infra/api/` as an unordered directory: Deployment pods consume the ConfigMap through `envFrom`, and a pod that starts with the previous value will not reload a later ConfigMap update. The observed value must exactly equal the verified runtime reference copied into the deployment registry before the Deployment is applied:

```sh
kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot apply -f infra/api/configmap.yaml
kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get configmap cma-api \
  -o jsonpath='{.data.CHRONO_SANDBOX_IMAGE}{"\n"}'
# Compare the printed value with the copied, digest-verified runtime reference.
```

For an ordinary release after the first-production cutover is complete, apply the new Deployment only after that digest comparison:

```sh
kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot apply -f infra/api/deployment.yaml
```

For the first production release containing the MongoDB cutovers below, do not run that Deployment command yet. Applying it before recovery capture A could start normal CMA and cross both database boundaries. Section 5 applies that Deployment only after the old writer is gone, capture A is durable, and the live ConfigMap selects the maintenance stage.

### Trigger native-provider adoption

Publishing the CMA Trigger code and runtime image does not activate its native
provider lane. Follow the ordered procedure in
[`CMA_Trigger_Operations.md`](CMA_Trigger_Operations.md#dedicated-provider-activation)
only after the CMA-owned provider tenant, native template, workload namespace,
API key, and infrastructure owner are independently verified. The ordinary CMA
provider and the neighbouring Aevatar Codex provider are not substitutes.

Keep `CHRONO_TRIGGER_ENABLED=false`, orchestration paused, the capability kill
switch enabled, runtime capabilities empty, and rollout allowlists closed while
applying the namespaced reclaimer RBAC, dedicated secret-provider projection,
mount, and complete provider tuple. Apply and verify each prerequisite before
restarting the Deployment; never use an unordered `kubectl apply -f infra/api/`
for this adoption. After restart, prove the versioned Trigger kernel route,
successful creation and fenced teardown, and zero total `BatchSandbox` objects
in the configured namespace. Only then advance rollout controls one at a time.

Before advancing any Trigger rollout control, provision the protected
`CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE` variable as an exact, reviewed match for
the deployed `CHRONO_TRIGGER_SANDBOX_NAMESPACE`, and provision
`CMA_TRIGGER_SMOKE_KUBECONFIG_B64` as the protected secret for a separate smoke
observer. Its namespace-local, list-only Role must grant `list` on
`batchsandboxes.sandbox.opensandbox.io`; do not reuse CMA's production
`get`/`list`/`delete` reclaimer identity. The provider-smoke workflow must
materialize the kubeconfig temporarily at mode `0600`, remove it under
`always()` before its evidence audit, and retain no Kubernetes credentials,
bearer tokens, client key/certificate data, or raw runtime reservation. A
passing smoke must provide complete inventory evidence of exactly zero
`BatchSandbox` objects before Launch, one entitled live object with zero
unexpected objects, and zero objects after session-stop reconciliation. The
retained allowlisted Kubernetes metadata and reservation SHA-256 must bind that
`0 before / 1 live / 0 after stop` result to the reviewed deployment before
rollout advances.

---

## 5. First production MongoDB cutover

The first production release containing these migrations needs **one continuous writer drain and two recovery units**. Capture A preserves the raw `sessions` and `session_events` collections for wholesale rollback to the old image. Capture B preserves verified, UUID-bound `sessions_v2` and `session_events_v2` before coordination can begin. Do not resume a database, object-storage, or provider writer between them.

Production must remain on `v0.7.0` until operators have proved Atlas backup/PITR or snapshot readiness and a restore path, plus version retention and a matching restore path for `CHRONO_CONTEXT_BUCKET`. Record the evidence, recovery identifiers, timestamps, retention, and restore owner before starting. A backup policy that has not produced and restored a usable recovery point is not evidence.

Run this once for each existing production database:

1. Confirm the old deployment is healthy. Verify directly that `sessions` and `session_events` are writable collections, record their UUIDs and document counts, and confirm `sessions_v2`, `sessions_v3`, `session_events_v2`, `session_coordination_receipts`, and `_cma_session_namespace_cutovers` are absent. If the source is not exactly raw legacy state, stop and investigate; do not adapt this procedure around it.
2. Fence ordinary traffic, then stop every old CMA replica and prevent it from returning. First prove that no pod carries the dedicated no-match label. Replace the Service's exact checked selector with that label and prove that its EndpointSlices contain no endpoint whose readiness is true or unknown. The JSON Patch `test` operation makes this fail closed if the live selector has drifted; investigate drift instead of editing the expected value during the release. Keep this Service fence unchanged through both captures, coordination activation, and the final unflagged restart. It covers both the direct API hostname and the app-origin Gateway routes because both route through `Service/cma-api`.

   ```sh
   (
     set -eu
     cma_fence_matches="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod \
       -l 'release.chrono-ai.fun/cutover-fence=mongodb-715' -o name)"
     if [ -n "${cma_fence_matches}" ]; then
       echo "the cutover fence selector unexpectedly matches a pod" >&2
       exit 1
     fi
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch service cma-api --type=json \
       -p='[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]'
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get service cma-api -o json | \
       jq -e '.spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}'
     cma_fence_attempt=0
     until kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-api -o json | \
       jq -e '[.items[].endpoints[]? | select(.conditions.ready != false)] | length == 0' >/dev/null; do
       cma_fence_attempt=$((cma_fence_attempt + 1))
       if [ "${cma_fence_attempt}" -ge 60 ]; then
         echo "cma-api still has a routable EndpointSlice endpoint; leaving traffic fenced" >&2
         exit 1
       fi
       sleep 2
     done
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot scale deployment/cma-api --replicas=0
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait --for=delete pod \
       -l app.kubernetes.io/name=cma-api --timeout=120s
   )
   ```

   The subshell refuses to scale down until EndpointSlice reconciliation has proved the traffic fence, and exits after 120 seconds without advancing if that proof never arrives. Then wait for every outstanding MongoDB operation, object mutation, and external provider create, delete, or replacement call to finish. Rotate or revoke the old workload's database, storage, and provider credentials where the deployment permits that stronger fence. If any later step fails, leave the no-match Service selector in place until an explicit incident decision is complete. Do not install a shell trap or other automatic cleanup that could reopen traffic after a failed release.

3. **Capture recovery unit A.** At this zero-writer boundary, take one transaction-consistent MongoDB backup or snapshot containing the raw collections and a version-retained recovery point for `CHRONO_CONTEXT_BUCKET`. Record both recovery identifiers and timestamps as one unit. If object storage is deliberately unconfigured, record that fact with the MongoDB recovery point. Prove the capture completed successfully before continuing.
4. Select the explicit legacy maintenance stage in the live ConfigMap, then apply the new API Deployment. The tuple is exact: stage `legacy`, legacy acknowledgement `drained`, and an empty coordination acknowledgement. Applying the Deployment is safe only after capture A because its first new pod now has no normal-startup path. Run this in a dedicated shell that remains open through capture B so the exact pod identity selected here remains available to step 5.

   ```sh
   set -eu
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch configmap cma-api --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"legacy","CHRONO_MONGODB_LEGACY_CUTOVER":"drained","CHRONO_MONGODB_COORDINATION_CUTOVER":""}}'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot apply -f infra/api/deployment.yaml
   cma_stage_pod_attempt=0
   while :; do
     cma_stage_pods_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-api -o json)"
     cma_stage_pod_count="$(printf '%s' "${cma_stage_pods_json}" | jq -er '.items | length')"
     case "${cma_stage_pod_count}" in
       0)
         cma_stage_pod_attempt=$((cma_stage_pod_attempt + 1))
         if [ "${cma_stage_pod_attempt}" -ge 60 ]; then
           echo "no cma-api stage pod appeared within two minutes; traffic remains fenced" >&2
           exit 1
         fi
         sleep 2
         ;;
       1)
         cma_stage_pod="$(printf '%s' "${cma_stage_pods_json}" | jq -er '.items[0].metadata.name')"
         cma_stage_pod_uid="$(printf '%s' "${cma_stage_pods_json}" | jq -er '.items[0].metadata.uid')"
         break
         ;;
       *)
         echo "more than one cma-api stage pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait \
     --for='jsonpath={.status.containerStatuses[?(@.name=="api")].started}=true' \
     "pod/${cma_stage_pod}" --timeout=31m
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_stage_pod}" -o json | \
     jq -e --arg uid "${cma_stage_pod_uid}" \
     '.metadata.uid == $uid and (.status.containerStatuses // [] | map(select(.name == "api"))) as $api | ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0'
   ```

   The creation gate waits up to two minutes for one pod and fails immediately if it observes more than one. Do not use `kubectl rollout status` for this stage. The Deployment cannot become Available because its sole pod is deliberately never Ready. The command instead waits up to 31 minutes for the exact captured `api` pod to pass its 30-minute startup probe, then rejects replacement or restart. Liveness and readiness remain suppressed until the post-shutdown `/livez` listener binds. A timeout or restart means the boundary was not reached and must be investigated with the Service fence still in place, not treated as capture B.
5. Verify the maintenance boundary before capture B. Recheck the exact API pod captured in step 4 and port-forward directly to it in that dedicated terminal. This bypasses the intentionally fenced Service. Leave the port-forward running while performing the HTTP checks, then stop it manually; its exit, replacement, or restart is a failed verification, not permission to continue.

   ```sh
   : "${cma_stage_pod:?step 4 did not capture the stage pod}"
   : "${cma_stage_pod_uid:?step 4 did not capture the stage pod UID}"
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_stage_pod}" -o json | \
     jq -e --arg uid "${cma_stage_pod_uid}" \
     '.metadata.uid == $uid and (.status.containerStatuses // [] | map(select(.name == "api"))) as $api | ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot port-forward \
     "pod/${cma_stage_pod}" 18080:8080
   ```

   From another terminal, mechanically assert the exact maintenance responses. The live route must return body `ok` with `200`. `/ready` and the real session collection route must return the exact staged reason with `503`, while `HEAD /livez` must also return `503`.

   ```sh
   (
     set -eu
     cma_stage_unavailable='{"status":"not-ready","reason":"legacy-cutover-staged"}|503'
     test "$(curl --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/livez)" = 'ok|200'
     test "$(curl --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/ready)" = "${cma_stage_unavailable}"
     test "$(curl --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/api/v1/sessions)" = "${cma_stage_unavailable}"
     test "$(curl --head --show-error --silent --max-time 10 --output /dev/null --write-out '%{http_code}' \
       http://127.0.0.1:18080/livez)" = '503'
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get service cma-api -o json | \
       jq -e '.spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}'
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-api -o json | \
       jq -e '[.items[].endpoints[]? | select(.conditions.ready != false)] | length == 0'
   )
   ```

   Directly in MongoDB, prove all of the following:

   - `sessions` is an empty-pipeline view of the sole writable `sessions_v2` collection, and `session_events` is an empty-pipeline view of the sole writable `session_events_v2` collection.
   - Both v2 collections have their exact strict/error validators. Their UUIDs equal the pre-rename UUIDs recorded at capture A.
   - The protocol-1 `sessions-v2` marker is complete, has exact origin `operator-drained`, and names the live UUIDs of `sessions_v2`, `session_events_v2`, and `session_tombstones`. Read its exact binary `collection_uuids.session_tombstones` value and assert that it equals the live `info.uuid` returned for `session_tombstones` by `listCollections`; a matching name, validator, or document set is not UUID proof. Make the equivalent exact comparison for both v2 collections.
   - `sessions_v3`, `session_coordination_receipts`, coordination activation provenance, and receipt provenance are absent. Session documents carry no coordination protocol field.
   - `session_tombstones` exists with its exact stage contract and valid history. Session/event counts, granted skills, skill custody, context references, write receipts, tombstones, and unrelated collections reconcile with capture A.

   The stage has already dropped its derived MongoDB handles and explicitly shut down its client before `/livez` became available. Do not treat a running process, a `503` readiness response without the exact reason, or a partially migrated database as this boundary.
6. **Capture recovery unit B.** While that exact maintenance process remains live and unready, take a second transaction-consistent MongoDB backup or snapshot containing `sessions_v2`, `session_events_v2`, `_cma_session_namespace_cutovers`, `session_writes`, and `session_tombstones`, plus a matching version-retained `CHRONO_CONTEXT_BUCKET` recovery point. Record and prove this pair independently from capture A. Never combine MongoDB from one recovery unit with contexts, skill objects, handover objects, or object versions from the other.
7. Restart into coordination activation without releasing the drain. Atomically clear the stage and legacy acknowledgement and set only the coordination acknowledgement to `drained`. Delete the maintenance pod completely before starting exactly one normal activation candidate. The Service must still carry the no-match selector from step 2. Run this in a dedicated shell that remains open through step 8 so the candidate cannot be replaced by a later label match.

   ```sh
   set -eu
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch configmap cma-api --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"","CHRONO_MONGODB_LEGACY_CUTOVER":"","CHRONO_MONGODB_COORDINATION_CUTOVER":"drained"}}'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot scale deployment/cma-api --replicas=0
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-api --timeout=120s
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot scale deployment/cma-api --replicas=1
   cma_activation_pod_attempt=0
   while :; do
     cma_activation_pods_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-api -o json)"
     cma_activation_pod_count="$(printf '%s' "${cma_activation_pods_json}" | jq -er '.items | length')"
     case "${cma_activation_pod_count}" in
       0)
         cma_activation_pod_attempt=$((cma_activation_pod_attempt + 1))
         if [ "${cma_activation_pod_attempt}" -ge 60 ]; then
           echo "no cma-api activation pod appeared within two minutes; traffic remains fenced" >&2
           exit 1
         fi
         sleep 2
         ;;
       1)
         cma_activation_pod="$(printf '%s' "${cma_activation_pods_json}" | jq -er '.items[0].metadata.name')"
         cma_activation_pod_uid="$(printf '%s' "${cma_activation_pods_json}" | jq -er '.items[0].metadata.uid')"
         break
         ;;
       *)
         echo "more than one cma-api activation pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait \
     --for=condition=Ready "pod/${cma_activation_pod}" --timeout=31m
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_activation_pod}" -o json | \
     jq -e --arg uid "${cma_activation_pod_uid}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   ```

   The bounded creation gate refuses multiple candidates, and the 31-minute wait covers the manifest's 30-minute startup-probe budget for only the captured pod. A timeout, replacement, or restart does not cancel or roll back database work: leave the Service fenced and every old writer stopped while investigating the candidate.
8. Recheck the exact Ready activation pod captured in step 7 and port-forward directly to it. Reject replacement or a startup restart before accepting the candidate.

   ```sh
   : "${cma_activation_pod:?step 7 did not capture the activation pod}"
   : "${cma_activation_pod_uid:?step 7 did not capture the activation pod UID}"
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_activation_pod}" -o json | \
     jq -e --arg uid "${cma_activation_pod_uid}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot port-forward \
     "pod/${cma_activation_pod}" 18080:8080
   ```

   Through that direct connection, run this exact normal-mode HTTP and fence gate from another terminal:

   ```sh
   (
     set -eu
     test "$(curl --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/livez)" = 'ok|200'
     test "$(curl --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/ready)" = '{"status":"ready"}|200'
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get service cma-api -o json | \
       jq -e '.spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}'
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-api -o json | \
       jq -e '[.items[].endpoints[]? | select(.conditions.ready != false)] | length == 0'
   )
   ```

   Perform the representative authenticated reads through that same port-forward; do not probe through the fenced Service. Verify directly that `sessions` views `sessions_v2`, `sessions_v2` views the sole writable `sessions_v3`, and the original `sessions_v2` UUID moved with the rename to `sessions_v3`. Confirm the strict session validator admits only `_cma_write_protocol: 2` with `_cma_coordination_protocol: 3`, every session carries both exact values, receipts carry write protocol 1 with coordination protocol 2, both coordination provenance records are complete and UUID-bound, and preserved history/custody/context/write/tombstone state reconciles with capture B. Stop the port-forward manually after verification. Any failure leaves traffic fenced.
9. Perform the final unflagged restart while ordinary traffic remains disabled. Clear the coordination acknowledgement, delete the flagged pod completely, start one replacement, and verify that its creation follows the ConfigMap change. Run the whole step in a dedicated shell that remains open through step 10; the pod identity captured before the Ready wait is the only candidate that may later receive traffic.

   ```sh
   set -eu
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch configmap cma-api --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"","CHRONO_MONGODB_LEGACY_CUTOVER":"","CHRONO_MONGODB_COORDINATION_CUTOVER":""}}'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot scale deployment/cma-api --replicas=0
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-api --timeout=120s
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot scale deployment/cma-api --replicas=1
   cma_final_pod_attempt=0
   while :; do
     cma_final_pods_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-api -o json)"
     cma_final_pod_count="$(printf '%s' "${cma_final_pods_json}" | jq -er '.items | length')"
     case "${cma_final_pod_count}" in
       0)
         cma_final_pod_attempt=$((cma_final_pod_attempt + 1))
         if [ "${cma_final_pod_attempt}" -ge 60 ]; then
           echo "no final cma-api pod appeared within two minutes; traffic remains fenced" >&2
           exit 1
         fi
         sleep 2
         ;;
       1)
         cma_final_pod="$(printf '%s' "${cma_final_pods_json}" | jq -er '.items[0].metadata.name')"
         cma_final_pod_uid="$(printf '%s' "${cma_final_pods_json}" | jq -er '.items[0].metadata.uid')"
         break
         ;;
       *)
         echo "more than one final cma-api pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot wait \
     --for=condition=Ready "pod/${cma_final_pod}" --timeout=31m
   cma_final_pod_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_final_pod}" -o json)"
   cma_final_pod_ip="$(printf '%s' "${cma_final_pod_json}" | jq -er '.status.podIP')"
   printf '%s' "${cma_final_pod_json}" | \
     jq -e --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and .status.podIP == $ip and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   ```

   The bounded creation gate fails immediately on multiple candidates. A timeout, replacement, or restart again leaves traffic fenced and does not prove that database work stopped. Port-forward directly to the already captured and verified pod from the same shell:

   ```sh
   : "${cma_final_pod:?step 9 did not capture the verified final pod}"
   : "${cma_final_pod_uid:?step 9 did not capture the verified final pod UID}"
   : "${cma_final_pod_ip:?step 9 did not capture the verified final pod IP}"
   cma_final_pod_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_final_pod}" -o json)"
   printf '%s' "${cma_final_pod_json}" | \
     jq -e --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and .status.podIP == $ip and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot port-forward \
     "pod/${cma_final_pod}" 18080:8080
   ```

   Verify all three live cutover values are empty and the same protected UUIDs and contracts remain. In the other terminal, rerun the exact normal-mode HTTP and fence gate from step 8 and perform the representative authenticated reads through the port-forward. Stop the port-forward manually but keep this shell and its three `cma_final_pod*` identity variables before restoring traffic.

10. Restore ordinary traffic only after every final check has passed. Export a short-lived release access token as `CHRONO_RELEASE_ACCESS_TOKEN` in the step 9 shell before continuing; the script requires it while traffic is still fenced and unsets it after successful restoration and authenticated verification. Replace the exact fence selector with the source selector by another compare-and-replace JSON Patch, wait for EndpointSlice reconciliation, require exactly one ready-or-unknown endpoint for the verified pod, and verify authenticated reads through both the direct API hostname and the app-origin API route. The installed traps only re-fence traffic; they never reopen it.

   ```sh
   set -eu
   : "${cma_final_pod:?step 9 did not record the verified final pod}"
   : "${cma_final_pod_uid:?step 9 did not record the verified final pod UID}"
   : "${cma_final_pod_ip:?step 9 did not record the verified final pod IP}"
   : "${CHRONO_RELEASE_ACCESS_TOKEN:?export the short-lived release access token while traffic is fenced}"
   cma_read_selector_state() {
     cma_selector_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get service cma-api -o json)" || return 1
     printf '%s' "${cma_selector_json}" | jq -er '
       if .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} then "fenced"
       elif .spec.selector == {"app.kubernetes.io/name":"cma-api"} then "source"
       else "unknown"
       end'
   }
   cma_wait_for_fenced_endpoints() {
     cma_fence_endpoint_attempt=0
     while :; do
       if ! cma_fence_endpoints_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get endpointslice \
         -l kubernetes.io/service-name=cma-api -o json)"; then
         echo "CRITICAL: cma-api is selected as fenced, but its EndpointSlices could not be read; keep or impose a manual traffic fence" >&2
         return 1
       fi
       if ! cma_fence_endpoint_count="$(printf '%s' "${cma_fence_endpoints_json}" | \
         jq -er '[.items[].endpoints[]? | select(.conditions.ready != false)] | length')"; then
         echo "CRITICAL: cma-api EndpointSlices could not be parsed after re-fencing; keep or impose a manual traffic fence" >&2
         return 1
       fi
       if [ "${cma_fence_endpoint_count}" -eq 0 ]; then
         return 0
       fi
       cma_fence_endpoint_attempt=$((cma_fence_endpoint_attempt + 1))
       if [ "${cma_fence_endpoint_attempt}" -ge 60 ]; then
         echo "CRITICAL: cma-api still has routable endpoints after re-fencing; impose a manual traffic fence" >&2
         return 1
       fi
       if ! sleep 2; then
         echo "CRITICAL: the re-fence EndpointSlice wait was interrupted; verify or impose a manual traffic fence" >&2
         return 1
       fi
     done
   }
   cma_refence_api() {
     cma_refence_patch_status=0
     kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch service cma-api --type=json \
       -p='[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]' || cma_refence_patch_status=$?
     if ! cma_refence_state="$(cma_read_selector_state)"; then
       echo "CRITICAL: could not read cma-api after the source-to-fence patch; impose a manual traffic fence" >&2
       return 1
     fi
     case "${cma_refence_state}" in
       fenced) ;;
       source)
         echo "CRITICAL: cma-api still has its source selector after the re-fence attempt; impose a manual traffic fence" >&2
         return 1
         ;;
       *)
         echo "CRITICAL: cma-api has an unknown selector after the re-fence attempt; impose a manual traffic fence" >&2
         return 1
         ;;
     esac
     if ! cma_wait_for_fenced_endpoints; then
       return 1
     fi
     if [ "${cma_refence_patch_status}" -ne 0 ]; then
       echo "the re-fence patch reported failure, but the exact fence selector and zero routable endpoints were verified" >&2
     fi
     return 0
   }
   cma_abort_after_reopen() {
     echo "$1; re-fencing cma-api before exit" >&2
     if cma_refence_api; then
       cma_traffic_open=0
       echo "cma-api is fenced again" >&2
       return 0
     fi
     echo "CRITICAL: automatic re-fencing could not be proved; impose and verify a manual traffic fence" >&2
     return 1
   }
   cma_refence_on_exit() {
     cma_exit_status=$?
     trap - EXIT
     if [ "${cma_traffic_open}" -eq 1 ]; then
       cma_refence_api || echo "CRITICAL: exit-time re-fencing could not be proved; impose and verify a manual traffic fence" >&2
     fi
     exit "${cma_exit_status}"
   }
   cma_refence_on_signal() {
     trap - HUP INT TERM
     echo "traffic-restoration shell interrupted; re-fencing before exit" >&2
     exit 1
   }
   cma_traffic_open=0
   trap cma_refence_on_exit EXIT
   trap cma_refence_on_signal HUP INT TERM
   kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get pod "${cma_final_pod}" -o json | \
     jq -e --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" \
     '.metadata.uid == $uid and .status.podIP == $ip and (.status.containerStatuses // [] | map(select(.name == "api" and .started == true and .restartCount == 0)) | length) == 1 and (.status.conditions // [] | map(select(.type == "Ready" and .status == "True")) | length) == 1'
   test "$(cma_read_selector_state)" = 'fenced'
   cma_traffic_open=1
   if ! kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot patch service cma-api --type=json \
     -p='[{"op":"test","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}},{"op":"replace","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}}]'; then
     if ! cma_reopen_state="$(cma_read_selector_state)"; then
       echo "CRITICAL: the reopen patch failed and the live selector could not be read; impose and verify a manual traffic fence" >&2
       exit 1
     fi
     case "${cma_reopen_state}" in
       fenced)
         if cma_wait_for_fenced_endpoints; then
           cma_traffic_open=0
           echo "the reopen patch failed and cma-api remains fenced" >&2
         else
           echo "CRITICAL: the reopen patch failed and a zero-endpoint fence could not be proved" >&2
         fi
         ;;
       source)
         if cma_refence_api; then
           cma_traffic_open=0
           echo "the reopen patch reported failure after applying the source selector; cma-api is fenced again" >&2
         else
           echo "CRITICAL: the reopen patch reported failure after applying the source selector, and automatic re-fencing could not be proved" >&2
         fi
         ;;
       *)
         echo "CRITICAL: the reopen patch failed and cma-api has an unknown selector; impose and verify a manual traffic fence" >&2
         ;;
     esac
     exit 1
   fi
   if ! cma_reopen_state="$(cma_read_selector_state)"; then
     cma_abort_after_reopen "the post-patch selector could not be read" || :
     exit 1
   fi
   if [ "${cma_reopen_state}" != 'source' ]; then
     cma_abort_after_reopen "the post-patch selector is not the exact source selector" || :
     exit 1
   fi
   cma_endpoint_attempt=0
   while :; do
     if ! cma_endpoints_json="$(kubectl --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-api -o json)"; then
       cma_abort_after_reopen "the reopened EndpointSlices could not be read" || :
       exit 1
     fi
     if ! cma_endpoint_state="$(printf '%s' "${cma_endpoints_json}" | \
       jq -r --arg pod "${cma_final_pod}" --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" '
       [.items[].endpoints[]? | select(.conditions.ready != false)] as $ready |
       if ($ready | length) == 0 then "pending"
       elif ($ready | length) == 1 and $ready[0].targetRef.kind == "Pod" and $ready[0].targetRef.name == $pod and $ready[0].targetRef.uid == $uid and (($ready[0].addresses | index($ip)) != null) then "exact"
       else "wrong"
       end')"; then
       cma_abort_after_reopen "the reopened EndpointSlices could not be parsed" || :
       exit 1
     fi
     case "${cma_endpoint_state}" in
       exact) break ;;
       wrong)
         cma_abort_after_reopen "a routable endpoint is not the exact verified pod" || :
         exit 1
         ;;
       pending) ;;
       *)
         cma_abort_after_reopen "the EndpointSlice gate returned an unknown state" || :
         exit 1
         ;;
     esac
     cma_endpoint_attempt=$((cma_endpoint_attempt + 1))
     if [ "${cma_endpoint_attempt}" -ge 60 ]; then
       cma_abort_after_reopen "the Service did not converge on the exact verified pod" || :
       exit 1
     fi
     if ! sleep 2; then
       cma_abort_after_reopen "the EndpointSlice wait was interrupted" || :
       exit 1
     fi
   done
   if ! test "$(curl --fail --show-error --silent --max-time 15 --write-out '|%{http_code}' \
     https://bot-api.chrono-ai.fun/livez)" = 'ok|200'; then
     cma_abort_after_reopen "the direct public liveness check failed" || :
     exit 1
   fi
   if ! test "$(curl --fail --show-error --silent --max-time 15 --write-out '|%{http_code}' \
     https://bot-api.chrono-ai.fun/ready)" = '{"status":"ready"}|200'; then
     cma_abort_after_reopen "the direct public readiness check failed" || :
     exit 1
   fi
   if ! curl --fail --show-error --silent --max-time 15 --output /dev/null \
     https://bot.chrono-ai.fun/openapi.json; then
     cma_abort_after_reopen "the app-origin discovery check failed" || :
     exit 1
   fi
   if ! cma_direct_sessions="$(curl --fail --show-error --silent --max-time 15 \
     --header "Authorization: Bearer ${CHRONO_RELEASE_ACCESS_TOKEN}" \
     'https://bot-api.chrono-ai.fun/api/v1/sessions?limit=1')"; then
     cma_abort_after_reopen "the authenticated direct API read failed" || :
     exit 1
   fi
   if ! printf '%s' "${cma_direct_sessions}" | jq -e 'type == "array"' >/dev/null; then
     cma_abort_after_reopen "the authenticated direct API read returned a non-session response" || :
     exit 1
   fi
   if ! cma_app_sessions="$(curl --fail --show-error --silent --max-time 15 \
     --header "Authorization: Bearer ${CHRONO_RELEASE_ACCESS_TOKEN}" \
     'https://bot.chrono-ai.fun/api/v1/sessions?limit=1')"; then
     cma_abort_after_reopen "the authenticated app-origin API read failed" || :
     exit 1
   fi
   if ! printf '%s' "${cma_app_sessions}" | jq -e 'type == "array"' >/dev/null; then
     cma_abort_after_reopen "the authenticated app-origin API read returned a non-session response" || :
     exit 1
   fi
   unset CHRONO_RELEASE_ACCESS_TOKEN cma_direct_sessions cma_app_sessions
   cma_traffic_open=0
   trap - EXIT HUP INT TERM
   ```

   A nonzero response from the reopen patch is never interpreted as proof that traffic stayed fenced. The script reads the live selector: an observed source selector is immediately changed back with an exact source-to-fence patch, an observed fence is verified through zero routable endpoints, and an unreadable or unknown selector is a critical manual-fence condition. After reopening, a command error, an unexpected routable endpoint, a timeout, a signal, or any public or authenticated verification failure takes the same fail-closed path. Only the exact pod name, UID, and address recorded in step 9 can pass the EndpointSlice gate. After both authenticated reads succeed, the success path unsets `CHRONO_RELEASE_ACCESS_TOKEN` and the captured response bodies before it marks restoration complete and removes the re-fence traps.

If any stage fails, keep every writer stopped. Before coordination begins, recovery unit A supports a whole-unit restore to the old image and recovery unit B supports a whole-unit restart from the proven v2 source. After coordination begins, never restore an old process against the advanced database and never splice the two recovery units. Make an explicit incident decision to restore one complete MongoDB/object unit or continue forward from its exact supported state.

---

## 6. MongoDB cutover contracts and recovery

The legacy migration records exact `operator-drained` intent and the original `sessions` and `session_events` UUIDs before its first protected mutation. A raw stage refuses a fresh database, an existing or ambiguous provenance namespace, a markerless v2 namespace, any unknown validator/action/level, and every coordinated state. An exact in-progress marker resumes only enumerated intermediate contracts while retaining those source identities. Before reconciliation can write a tombstone, the stage binds the exact `session_tombstones` collection UUID. Completion majority-writes the UUIDs of `sessions_v2`, `session_events_v2`, and `session_tombstones`. Re-entering the stage after completion is read-only verification: before and after the completed-stage rerun, assert that the completed marker's exact binary `collection_uuids.session_tombstones` value equals the live `listCollections` `info.uuid` for `session_tombstones` and that neither value changed. The rerun refuses a missing or replacement tombstone collection, does not replace a validator or repair provenance, and never adopts a replacement UUID.

Coordination activation moves the writable session collection from `sessions_v2` to `sessions_v3` while leaving `session_events_v2` in place. It also advances the session writer to `_cma_write_protocol: 2` with `_cma_coordination_protocol: 3`, preventing an older whole-document replacement from silently removing `granted_skills`, `skill_packs`, or other current fields. `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` is the operator assertion that every older database, object-storage, and provider operation has permanently stopped; CMA cannot discover that external fact.

A genuinely empty database activates automatically. Any database that existed before startup requires the exact acknowledgement, even when its session collections happen to be empty. Activation majority-writes intent before fencing and renaming `sessions_v2`; the rename preserves its collection UUID. It then creates `sessions_v2` as a view of `sessions_v3`, establishes and audits the receipt collection, and completes a marker bound to both protected UUIDs. A completed marker authorizes verification only: startup will not recreate a missing session or receipt collection, repair a replacement collection, or infer a new empty store.

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

**The runtime D-09 gate failed or was inconclusive.** Inspect the
`cma-trigger-d09-release-<run>-<attempt>` artifact before retrying. The gate
initializes a closed inconclusive record before checkout, artifact download, or
registry access, so a missing or malformed campaign result is still evidence of
where the release stopped. Re-run failed jobs only after the recorded failure is
understood; the rerun consumes the same immutable runtime export while it is
retained. Do not delete, move, or republish either GHCR tag to make the gate
pass. If the runtime OCI artifact has expired, use a whole-workflow rerun under
the required writer and package-history fences.

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
