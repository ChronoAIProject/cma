# Release

A release is a merge into `main`. There is no separate step before it and none after it: the merge decides the version, publishes the images and writes the record.

This document is the procedure. The rules it implements are in [CLAUDE.md](../CLAUDE.md) §6.

---

## 1. What a release produces

| Artifact                                       | Where                                                                        | Integrity contract                                                      |
| ---------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| `ghcr.io/chronoaiproject/cma-api:vX.Y.Z`       | GHCR, private                                                                | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-web:vX.Y.Z`       | GHCR, private                                                                | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-runtime:vX.Y.Z`   | GHCR, private                                                                | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-cmaeg:vX.Y.Z` | GHCR, private | intended write-once; requires audited writer access and package history |
| `ghcr.io/chronoaiproject/cma-image-manager:vX.Y.Z` | GHCR, private | intended write-once; requires audited writer access and package history |
| the same five at `:<short-sha>`               | GHCR, private                                                                | intended write-once under the same audited contract                     |
| five OCI transfer archives with JSON metadata | the `Publish release` Actions run, readable under repository artifact access | yes; retained for up to one day from upload                             |
| closed D-09 runtime-boundary evidence          | the same Actions run, readable under repository artifact access              | exact release commit and runtime digest; retained for 90 days           |
| annotated tag `vX.Y.Z`                         | this repository                                                              | write-protected after creation by the required active ruleset           |
| GitHub release `vX.Y.Z`                        | this repository                                                              | notes editable                                                          |

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

| Rule                                    | Rejected example                              |
| --------------------------------------- | --------------------------------------------- |
| The section exists, exactly once        | no `## Release Notes` heading, or two of them |
| All three subsections, in this order    | `What's Fixed` placed before `What's New`     |
| Spelled and capitalised exactly         | `### What's new`                              |
| Plain apostrophe, not a typographic one | `### What’s New`                              |
| At least one bullet under each          | a subsection left empty                       |
| Only bullets underneath                 | `We added things.` as a paragraph             |
| No version number                       | `0.1.12` under the heading                    |

Bullets may be `-` or `*`, and may nest. The section ends at the next `##` heading, so anything meant for the reviewer goes outside it and is never published.

**Do not write a version number anywhere.** The label decides how far the version moves; the publisher computes the number and writes it in as `## Release Notes vX.Y.Z`. A number typed into the pull request would be a guess at what the merge decides, so it is rejected rather than trusted.

The `Release declaration` check re-runs when a label or the body changes, so fixing either clears it without pushing a commit. `.github/release-notes.sh` is what reads the section, and the publisher runs the same script on the same body after the merge, so what ships is what was reviewed.

### Required checks on `main`

Branch protection on `main` requires these contexts, and `enforce_admins` is on — a required check that nothing reports leaves the pull request unmergeable by anyone, with no override:

```
Repository structure   Secret scanning   Backend   Frontend   Manifests
Images (api)           Images (web)      Images (runtime)
Images (cmaeg)         Images (image-manager)
TypeScript SDK         Rust SDK          Source branch
Release declaration    Commit hygiene    Trigger release contract
Trigger provider smoke contract
```

A context is the job's `name:`, so **renaming a job is a branch-protection change.** Rename one without updating the list and every later pull request into `main` waits forever on a check that no longer exists. Update the two together, in the same rollout:

```sh
REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
gh api -X PATCH "repos/$REPOSITORY/branches/main/protection/required_status_checks" \
  -F strict=true -f 'contexts[]=Release declaration' ...
```

Use the `checks` form of the branch-protection API for the two Trigger jobs and
bind each to the same non-null GitHub Actions App ID required on `develop`.
Legacy context-only entries accept a status from any app with that name and do
not close this gate. On 2026-08-29, both `Trigger release contract` and
`Trigger provider smoke contract` were installed on `main` and independently
re-read from `required_status_checks.checks` with GitHub Actions App ID `15368`.
The release pull request must remain draft if either exact context/app binding
is later absent or drifts.

`Images (runtime)`, `TypeScript SDK`, and `Rust SDK` are required on both
protected branches. The SDK jobs prove the repository-only packages build and
work for source consumers; requiring them does not publish either SDK.
The service split adds `Images (cmaeg)` and `Images (image-manager)` to the target
required checks. Their remote branch-protection bindings and new package ACLs
still need operator verification; this local change has not modified GitHub settings.

### Required release write fences

These are hard prerequisites for merging a release, not recovery advice to
apply after publishing starts:

1. **GHCR has one audited ordinary-operation package writer.** On 2026-08-29,
   the authenticated package settings for private packages `cma-api`,
   `cma-web`, and `cma-runtime` were changed and re-read one package at a time.
   Repository permission inheritance is disabled on all three. Each package
   grants Actions access to exactly `ChronoAIProject/cma`
   (`repository_id=1330597394`) at `Write`, has zero direct package members or
   teams, and grants no other repository Actions access. The package version
   pages still contain the same `v0.7.0`, `v0.8.0`, and `v0.9.0` roots after
   that change. The organization default still enables inheritance for newly
   created packages; it does not change these three explicit ACLs, and every
   new release package requires its own audit before use.

   GitHub automatically grants package `Admin` to every organization owner and
   does not expose a package-level control that can remove it. The audited
   break-glass exceptions are `aelfops` (`182210179`), `chronoai-kai`
   (`296105127`), `chronoai-shining` (`250120269`), `ElonSG` (`293525633`),
   and `loning` (`1593871`). They are administrators, not ordinary release
   writers. Re-read the owner set and organization MFA enforcement before each
   release. Owner drift, a non-owner user/team/repository with `Write` or
   `Admin`, or an owner package operation without an approved incident/change
   record reopens this gate.

   The package audit history through this ACL change contains 24 CMA package
   publication events, all by `github-actions[bot]`, with no CMA package or
   package-version deletion event. Any future CMA publication actor other than
   `github-actions[bot]`, any deletion, or any unexplained version/tag movement
   reopens the gate. In source, only the `Publish release` `images` job receives
   `packages: write`, and every action executed in that job is pinned to a full
   commit SHA. Removing a pin, adding another package-write job/workflow, or
   enabling a standing human/token writer invalidates this evidence. Literal
   zero-human package administration is not a capability GHCR offers under an
   organization; if that stronger property is required, use a registry and
   ownership model that can enforce it.

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
- after all five images are in GHCR, exports each published digest to a one-day OCI transfer artifact without rebuilding it;
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

The release body lists the five GHCR references for direct consumers. When a deployment registry cannot authenticate to GHCR, use the OCI archives from the exact successful **Publish release** run. Each expires up to one day after its export upload, which can be earlier than the workflow's completion. Do not substitute `docker pull`, `docker tag`, `docker push`, or `docker save`: those paths do not prove that the destination kept the published root manifest and every platform manifest.

The transfer requires `gh`, `jq`, Docker, and OpenSSL. Run it from this repository's checkout, set the release version, and copy the numeric run ID from the successful run's Actions URL. The following checks that the run succeeded at the commit named by the release tag, derives the short revision from that commit, and downloads all five artifacts into fresh temporary directories:

```bash
set -euo pipefail

REPOSITORY="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
RUN_ID='<successful-publish-run-id>'
VERSION=vX.Y.Z
TRANSFER_DIR="$(mktemp -d)"
AUTH_DIR="$(mktemp -d)"
trap 'rm -rf "$TRANSFER_DIR" "$AUTH_DIR"' EXIT HUP INT QUIT TERM

run_json="$(gh run view "$RUN_ID" --repo "$REPOSITORY" \
  --json workflowName,conclusion,headSha,url)"
jq -e '.workflowName == "Publish release" and .conclusion == "success"' \
  <<<"$run_json"
HEAD_SHA="$(jq -r '.headSha' <<<"$run_json")"
test "$HEAD_SHA" = "$(gh api "repos/$REPOSITORY/commits/$VERSION" --jq '.sha')"
REVISION="${HEAD_SHA:0:7}"

for component in api web runtime cmaeg image-manager; do
  mkdir "$TRANSFER_DIR/$component"
  gh run download "$RUN_ID" --repo "$REPOSITORY" \
    --name "cma-${component}-${VERSION}-${REVISION}-oci" \
    --dir "$TRANSFER_DIR/$component"
done
```

Inspect and validate the JSON instead of sourcing it as shell code. This checks every selected identity, including the full and short commit revisions, the archive name and checksum, the source references, and the intended platform set. The exporter writes these files only after the version and revision tags have the same raw root-manifest digest, every source and archive platform config carries the exact version/full-revision labels, and the archive reproduces the root digest. `sourceDigest` is the GHCR digest observed and reproduced by the exporter at export time; accept it as the release identity only after the exclusive-writer and package-history checks above rule out a preseed or stale-read overwrite. Neither `archiveSha256` nor the checksum GitHub shows for its enclosing Actions artifact is an image digest.

```bash
for component in api web runtime cmaeg image-manager; do
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
trap - EXIT HUP INT QUIT TERM
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

**It especially does not move `CHRONO_SANDBOX_IMAGE`.** Under [CLAUDE.md](../CLAUDE.md) §5 that value _is_ the runtime version, and every session running a different one is rotated onto it — transcript snapshotted, sandbox replaced, agent resumed. Editing it is a rollout of every live session, which is why publishing a runtime image and adopting it are separate acts. Adopt one by editing `infra/api/configmap.yaml` in its own pull request, deliberately, when a rotation is what you want.

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

### Compressed checkpoint runtime adoption

The runtime can seal whole-home checkpoints as deterministic gzip containers.
Their object references and SHA-256 values identify the exact compressed bytes;
the internal manifest, file hashes, restore bounds, and runtime contract remain
unchanged. The compressed-checkpoint runtime restores both historical plain
archives and new compressed archives without an uncompressed archive staging
file.

Restoration verifies the archive SHA-256 and validates the complete manifest
before creating a staging directory. It then reads the payload once to extract
files, checking each file digest and the compression trailer before replacing
the Sandbox home. Upload publication still independently checks the full archive.
This avoids a duplicate restore decompression without changing the archive
format, integrity checks or runtime contract.

This is a forward-compatible adoption, not a reversible image-only change.
Once a Sandbox has retained a compressed checkpoint, do not roll its runtime
back to an image that only reads plain archives. Runtime contract 3 alone does
not prove this storage-format compatibility. Record the adopted immutable
runtime digest in deployment evidence, verify restoration of both archive
encodings, and retain a gzip-capable runtime for rollback. Never discard or
rewrite an owner's checkpoints to make an older image appear compatible.

### App-origin route and health-policy adoption

The release that adds network admission limits also changes two Gateway
contracts. App-origin API, auth, and discovery traffic must reach `cma-agent-manager`
directly so the trusted GFE forwarding chain, rather than the web pod, supplies
the client address used for anonymous and admission budgets. The load balancer
must probe `/ready`, not `/livez`, so a process that has lost its session store
is not externally routable merely because it is alive.

The legacy web image still proxies `/api/` and `/auth/` through
`Service/cma-agent-manager`, so the Service selector fence below covers both the legacy
nginx path and the new direct Gateway path. That fallback is not an acceptable
way to reopen the new API: its TCP peer is the untrusted web pod, which would
collapse browser network budgets onto one key. Do not broaden
`CHRONO_SERVER_TRUSTED_PROXY_CIDRS` to admit pod networks.

Adopt the owned route bundle and health policy as one reversible
production-routing change while the old API and web images are healthy, before
changing the runtime ConfigMap or starting the writer drain. The release
operator needs Kubernetes read/write access. The temporary read-only Compute
evidence role on project `example-project-02` must include
`compute.globalAddresses.get`, `compute.globalForwardingRules.get`,
`compute.targetHttpProxies.get`, `compute.targetHttpsProxies.get`,
`compute.urlMaps.get`, `compute.sslCertificates.get`,
`compute.backendServices.get`, `compute.healthChecks.get`,
`compute.networkEndpointGroups.get`,
`compute.networkEndpointGroups.listNetworkEndpoints`, and
`compute.sslPolicies.get` when the HTTPS proxy references an SSL policy. An
`Attached=True` Kubernetes policy condition is not enough: it is not
generation-bound and can still describe the old `/livez` attachment.

Those permissions are a pre-mutation evidence gate, not rollout authorization.
The current Gateway checker has an open partial effective-Compute predicate and
must not be treated as closing behavior-changing fields. Once the temporary
read role exists, capture the two live backend-service JSON documents, the API
health-check JSON, and the complete effective routing trace. The trace must bind
the exact Gateway UID, `resourceVersion`, annotations, and status IP through the
global address, the exact TCP port 80 and 443 forwarding rules, target HTTP/HTTPS
proxies, one redirect and one serving URL map, and the attached classic
certificate/SSL policy or a separately closed Certificate Manager chain. The
Gateway status IP, address, and both forwarding-rule IPs must be equal; the
forwarding rules must terminate at the exact proxies, which must terminate at
the exact redirect/routing maps with closed TLS, QUIC, and proxy-header settings.
It must resolve the exact `bot.chrono-ai.fun` and `bot-api.chrono-ai.fun`
host/path/backend and header actions without relying on generated-name heuristics.
All host rules must be scanned for CMA overlap and must exclude wildcard overlap,
weighted backends, or alternate header, rewrite, mirror, and fault actions; the
backend links must equal the API/web references from the bound Gateway
annotations. Normalize by stripping only observational metadata, then re-read
every Compute fingerprint plus the Gateway identity after capture.

Using that evidence, land a reviewed checker revision whose normalized
projections are closed over behavior. Backend projections must include CDN,
request/response headers, IAP and edge/backend security policies, session
affinity, connection draining, logging, load-balancing policy, every per-NEG
balancing/capacity/rate field, and every API and web health-check link. Health
projections must include interval, timeout, healthy/unhealthy thresholds,
proxy-header mode, host, request path, response, port specification, and logging.
The URL-map/proxy projection must close redirects, default services, host/path
rules, route rules, header actions, certificates, SSL policy, QUIC override, and
proxy-header settings. The same revision must validate the exact shared Gateway
specification, current-generation `Programmed` status and listener conditions,
then use a closed `cma-agent-manager`/`cma-web` Service projection as the root of each NEG
trace. That projection includes the desired `cloud.google.com/neg` annotation,
the complete accepted `cloud.google.com/neg-status` shape, Service type, ports,
traffic-policy/default fields, and the mode-specific selector: source selectors
during adoption/rollback checks, an explicit fenced-from-snapshot mode before
step 10 reopens traffic, and the source/adopted mode after selector restoration.
The live capture must also retain one nonempty
`list-network-endpoints --format=json` response for the verified pod. The closed
reopen projection must bind every behavior-bearing member field, including any
instance/zone identity, to the pod's captured node/provider identity; it may omit
that binding only if the captured response proves no such field exists. Stale NEG
status or backend links are not roots of trust. Exact expected values and fixtures
must come from the reviewed live capture, with a mutation test for every projected
field; do not invent absent/default values. Fail-closed evidence-collection and
checker tooling may merge, but production adoption must remain disabled and
`--adopt` must not be run until that closed projection and its tests are in the
exact reviewed release commit, even if the current partial checker returns
success.

Public DNS is not proof of the origin attachment graph. A read-only check on
2026-08-29 found both CMA hostnames proxied through Cloudflare while the GKE
Gateway reported origin address `136.69.78.214`; the address is an observation,
not a release constant, and must be recaptured from the closed projection. The
release gate therefore needs two independent probe lanes. Ordinary public-chain
probes must continue resolving the hostnames normally and exercise the complete
visitor-to-Cloudflare-to-GKE path. Separate origin-pinned HTTP and HTTPS probes
must use `curl --resolve` with the exact Gateway address produced by the same
coherent capture, preserve each hostname's Host/SNI identity, and require
`%{remote_ip}` to equal that captured address. Every release curl must put
`--disable` first, set `--noproxy '*'`, and run with `SSLKEYLOGFILE` removed so
operator curl configuration, inherited proxies, and TLS key logging cannot
change or disclose a probe. `CURL_CA_BUNDLE`, `SSL_CERT_FILE`, `SSL_CERT_DIR`,
and `CURL_SSL_BACKEND` must also be removed so an inherited trust root or TLS
backend cannot weaken the HTTPS proof. The checked program must consume the
captured value rather than an operator-entered lookalike. If either DNS
record remains proxied, the reviewed public-chain projection must additionally
close every behavior-bearing Cloudflare DNS, origin/TLS, redirect, cache,
transform, header, and routing setting that can affect these hostnames, freeze
competing writes through the probes, and re-read its versioned identities after
the probe set. A successful public response alone cannot authorize adoption or
traffic restoration.

The Cloudflare lane is a checked evidence campaign, not an operator-written
configuration summary. Run it only from the clean, reviewed release commit via
the trusted `env -i` launcher and the reviewed host toolchain. The checkout
gate rejects tracked changes, non-ignored untracked files, `assume-unchanged`
entries, and `skip-worktree` entries. Keep the checked-in
`cma-trigger-gateway-cloudflare-policy.json` at
`blocked-pending-reviewed-cloudflare-capture` until the baseline below has been
captured, retained, and reviewed. Keep
`cma-trigger-gateway-live-policy.json` at
`blocked-pending-reviewed-live-capture` until the separate Gateway projection
review is complete.

First mint a read-only Cloudflare API token that is active now, expires no more
than one hour after capture starts, and can read the zone, account, DNS,
settings, Rulesets, Page Rules, Workers routes/domains, Access applications,
Waiting Rooms, legacy firewall/rate-limit rules, load balancers, cache, snippets,
and origin-TLS roots enumerated by the capture program. Do not export it. Disable
xtrace and core dumps, read it privately, and pass it through protected
descriptor 7 using an anonymous pipe.

Create a new absolute off-host evidence directory and run the baseline capture.
Use the exact reviewed toolchain path for the release host; the illustrative
path below must be replaced rather than accepted as an approval of those
binaries.

```bash
set +x
if ! umask 077 || ! ulimit -c 0 || [ "$(ulimit -c)" != 0 ]; then
  echo 'private capture protections could not be established' >&2
  exit 1
fi
IFS= read -r -s CMA_CLOUDFLARE_CAPTURE_TOKEN
printf '%s\n' "${CMA_CLOUDFLARE_CAPTURE_TOKEN}" | (
  exec 7<&0
  exec 0</dev/null
  CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7 \
  CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
    .github/scripts/cma-trigger-gateway-launch.sh cloudflare --capture \
      '<new-absolute-off-host-directory>/cloudflare-baseline'
)
cma_cloudflare_capture_status=$?
CMA_CLOUDFLARE_CAPTURE_TOKEN=''
unset CMA_CLOUDFLARE_CAPTURE_TOKEN
test "${cma_cloudflare_capture_status}" -eq 0
unset cma_cloudflare_capture_status
```

The bundle is published atomically only after all 29 response roots, the
manifest, the exact receipt, and their durability barriers succeed. A denied,
partially readable, expired, or plan-gated endpoint is a failed capture; do not
interpret it as an empty feature. Keep the failed directory unpublished, fix
the token entitlement or review an exact plan-specific absence contract, mint
a fresh short-lived token, and rerun into another new directory. Retain the
successful read-only bundle off-host.

Independently record the exact receipt SHA-256 printed by the capture, verify it
against both the retained receipt and `receipt.sha256`, and derive the review
candidate from that exact bundle. Offline verification and derivation receive
no token descriptor.

```bash
cma_cloudflare_receipt='<absolute-off-host-directory>/cloudflare-baseline/receipt.json'
cma_cloudflare_receipt_sha256='<independently-recorded-receipt-SHA-256>'
CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256="${cma_cloudflare_receipt_sha256}" \
CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  .github/scripts/cma-trigger-gateway-launch.sh cloudflare --verify-receipt \
    "${cma_cloudflare_receipt}"
CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256="${cma_cloudflare_receipt_sha256}" \
CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  .github/scripts/cma-trigger-gateway-launch.sh cloudflare --derive-policy \
    "${cma_cloudflare_receipt}" >'<private-review-path>/cloudflare-policy-candidate.json'
```

The reviewer must compare the candidate with the retained capture and approve
its provenance. `sourceEvidenceSha256` must equal the independently recorded
receipt digest; `zoneId` and `accountId` must equal the exact captured numeric
identities; and `expected.behaviorProjection` plus `expected.settings` must be
the complete deterministic projection emitted by `--derive-policy`. The
reviewed policy is made consumable by changing only `status` from
`candidate-pending-review` to `approved`; hand edits to any provenance,
identity, setting, or projection require a fresh candidate and review. Commit
that approved object as `cma-trigger-gateway-cloudflare-policy.json`, record its
SHA-256, and retain the source bundle. The release contract additionally pins
`Pseudo IPv4` to Disabled and the official Cloudflare CIDR response to exactly
15 IPv4 and 7 IPv6 prefixes.

Before the public/origin campaign, establish and externally record a bounded
writer-freeze window covering every Cloudflare identity represented by the
policy. No human, API client, Terraform/GitOps process, Worker publisher, DNS
automation, or dashboard session may mutate those roots until the post-probe
capture is complete. With an approved closed Gateway source projection and its
Cloudflare policy, run:

```bash
set +x
if ! umask 077 || ! ulimit -c 0 || [ "$(ulimit -c)" != 0 ]; then
  echo 'private capture protections could not be established' >&2
  exit 1
fi
IFS= read -r -s CMA_CLOUDFLARE_CAPTURE_TOKEN
printf '%s\n' "${CMA_CLOUDFLARE_CAPTURE_TOKEN}" | (
  exec 7<&0
  exec 0</dev/null
  CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7 \
  CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  CMA_GATEWAY_CLOUDFLARE_POLICY_SHA256='<reviewed-policy-SHA-256>' \
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM=writers-disabled-through-origin-probes \
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID='<bounded-external-record-id>' \
  CMA_GATEWAY_ORIGIN_EVIDENCE_DIR='<new-absolute-off-host-directory>/origin-campaign' \
    .github/scripts/cma-trigger-gateway-launch.sh origin \
      '<absolute-approved-source-projection>/projection.json'
)
cma_origin_campaign_status=$?
CMA_CLOUDFLARE_CAPTURE_TOKEN=''
unset CMA_CLOUDFLARE_CAPTURE_TOKEN
test "${cma_origin_campaign_status}" -eq 0
unset cma_origin_campaign_status
```

The origin program creates its own fresh Cloudflare capture before the probe
window and another after it; an externally supplied old receipt cannot replace
either capture. It requires the pre/post zone, settings, and full behavior
projection to be identical, and verifies both proxied CMA A records bind to the
Gateway IP in the approved projection. Ordinary DNS probes exercise both public
hosts using a receipt-derived nonce, require verified TLS, a canonical
Cloudflare remote IP and `CF-Ray`, `Server: cloudflare`,
`CF-Cache-Status: DYNAMIC`, and no `Age` header. Independent origin-pinned
`curl --resolve` probes prove the same host/SNI identities and exact origin IP.
The source projection is checked against live state before and after the
campaign.

Any failed API call, probe, equality check, timeout, token check, or durability
barrier publishes no successful campaign directory. Keep writers frozen until
the failure and live state are understood; discard no retained evidence, mint a
new bounded token, and retry into a new path. A successful directory is
atomically published with `receipt.json` and `receipt.sha256`; retain it and
the baseline capture in durable off-host custody and link their reviewed
digests in the release evidence ledger.

These read-only programs close evidence only. The adopted projection, durable
pre-patch fence receipt, fail-closed `ValidatingAdmissionPolicy`, two consecutive
empty NEG sweeps, pre-reopen fenced projection, and final source projection are
still mandatory. The projection operator also needs
`compute.backendServices.getHealth` and `compute.firewalls.get` in addition to
the other enumerated read permissions. Until the approved live Gateway and
Cloudflare policies, client-IP authority branch, writer freezes, retained
receipts, admission drill, projection captures, and independent reviews are all
present, production mutation remains hard-disabled by the checked source
constant. Do not run adoption or traffic restoration merely because the
read-only campaigns pass.

Production is also blocked on live validation of the selected conditional
chain-aware client-IP authority contract. The deployed v0.7 configuration
selects the GFE-observed Cloudflare egress address and therefore collapses
anonymous/admission budgets across users. The release candidate implements the
replacement contract in source and deployment configuration: it trusts the
forwarded chain only from an approved GFE peer, requires the GFE-observed
upstream token to belong to the build-pinned complete official Cloudflare CIDR
set, and advances to the next-left visitor token only when that token exactly
matches the single valid `CF-Connecting-IP` header without a Worker or
Pseudo-IPv4-overwrite marker. Otherwise it uses the GFE-observed upstream and
ignores every caller-controlled prefix. Startup accepts only an empty
Cloudflare set or the exact build-pinned set and rejects overlap with the GFE
ranges.

That implementation closes the architecture choice, not the live evidence
gate. Before exposure, compare the official Cloudflare IPv4 and IPv6 sets with
the build pin in both directions and prove the reviewed Cloudflare behavior
preserves the required headers. Until the selected branch passes the live
Cloudflare header-shape drill plus direct-origin, spoofed-prefix,
non-Cloudflare-upstream, malformed-chain, CIDR-drift, IPv4, and IPv6 tests,
production adoption, `--adopt`, and step 10 traffic restoration remain disabled.

Run the checked adoption program from a completely clean checkout whose `HEAD`
is the exact reviewed 40-hex release commit. A branch, tag, abbreviated commit,
or checkout with any tracked or untracked change is refused. Before reading or
writing production, the program validates that the source bundle contains
exactly the three owned routes and one `/ready` health policy, emits four
canonical desired objects, derives its source seal from the reviewed commit
blobs, and refuses a final live aggregate that differs from that reviewed seal.
It continues rechecking the reviewed source and emitted-object seals through the
transaction.

Because Kubernetes cannot atomically predicate one health-policy write on three
route `resourceVersion`s, first establish an exclusive spec-writer window for
the entire checked routing graph. The shared Gateway currently has other
attached routes, so this window must freeze creation, deletion, and spec changes
for every `HTTPRoute` in any namespace allowed to attach to either the HTTP or
HTTPS listener, not only the three CMA routes. An admission-enforced host-
ownership policy that categorically rejects any new or changed route overlapping
`bot.chrono-ai.fun` or `bot-api.chrono-ai.fun` is the only acceptable substitute
for that all-attachable-route freeze. During adoption or restore, no human,
automation, or service may write directly to the three owned `HTTPRoute`s,
`HealthCheckPolicy/cma-agent-manager`, the shared `gateway-infra/chronoai-gateway` Gateway,
`Service/cma-agent-manager`, `Service/cma-web`, `GCPBackendPolicy/cma-agent-manager`,
`GCPBackendPolicy/cma-web`, or the generated Compute global address, forwarding
rules, target HTTP/HTTPS proxies, URL maps, certificates, SSL policy, backend
services, health checks, and network endpoint groups. Pause GitOps and other
release automation and
coordinate every identity with any of those write paths. Normal GKE Gateway
controller status and effective-resource reconciliation remains allowed; it is
not a competing direct writer. The required
`CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer` value is an operator
attestation, not an admission lock. Do not set it unless that window exists, and
keep the window through command completion. UID/resourceVersion checks and the
effective-state barriers still fail closed on detected drift.

First provision a durable off-host custody root for the rollback files. It must
be a pre-existing absolute directory owned by the release user, must not be a
symlink, and must have mode `0700`. The adoption program verifies those local
properties and requires the explicit `durable-off-host` attestation before it
reads any production object; the operator remains responsible for ensuring the
directory is synchronously durable outside the operator host. Rollback files are
created directly under that custody root, not copied there after mutation.

```sh
CMA_GATEWAY_RELEASE_REVISION='<reviewed-full-SHA>'
cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref \
  --points-at "${CMA_GATEWAY_RELEASE_REVISION}" \
  --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' 'refs/tags/v*')"
[[ "${cma_gateway_release_tag_record}" =~ ^(v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))\|tag\|commit\|${CMA_GATEWAY_RELEASE_REVISION}$ ]]
cma_gateway_release_tag="${BASH_REMATCH[1]}"
unset cma_gateway_release_tag_record
CMA_GATEWAY_RELEASE_REVISION="${CMA_GATEWAY_RELEASE_REVISION}" \
CMA_GATEWAY_CONFIRM="adopt-${cma_gateway_release_tag}-route" \
CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer \
CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT='<durable-absolute-directory>' \
CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host \
CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>' \
CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>' \
CMA_GATEWAY_PROJECTION_DIR='<absolute-reviewed-source-projection-directory>' \
CMA_GATEWAY_PROJECTION_SHA256='<externally-recorded-source-projection-SHA-256>' \
CMA_GATEWAY_ADOPTED_PROJECTION_PARENT='<absolute-off-host-projection-directory>' \
CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  .github/scripts/cma-trigger-gateway-launch.sh adoption --adopt
```

Run that command only in a release terminal whose output is durably recorded
outside the operator host. The program captures the four live source objects in
a private child of the attested custody root, file-fsyncs each object while
changing it to mode `0400`, fsyncs the custody directories where the filesystem
supports directory fsync, and only then prints one success acknowledgement with
the rollback path and `Gateway rollback SHA-256`, the adopted projection path
and SHA-256, and its pre-adoption source-projection predecessor SHA-256. Record
all five values in the external release record as soon as they appear and retain
both sealed directories through release completion. Carry the adopted digest as
`CMA_GATEWAY_SOURCE_PROJECTION_SHA256` and the predecessor digest as
`CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256` when fencing. Each digest
authenticates its retained artifact; it is not a substitute for that artifact.

Before its first write, the program accepts only the exact legacy or
already-adopted source pair, proves all three route generations and the effective
GCP health check/backend-to-NEG state, runs the complete old-image HTTP probe,
including an exact `{"status":"ready"}` response from the direct `/ready`
endpoint, rechecks every live UID and allowed spec, and server-side dry-runs all
four desired replacements plus all four sealed rollback replacements. Missing
Compute read permission, source drift, an unhealthy old image, or any preflight
error therefore stops before mutation.

Forward writes use `kubectl replace`, not apply. Each replacement carries the
freshly read UID and `resourceVersion`, so a concurrent change fails its
compare-and-swap instead of being overwritten. The fixed order is health policy,
direct API route, app-origin route, then HTTP redirect. After the health-policy
write and before the first route write, the program proves that the effective
GCP health check has changed to `/ready` while every route still exactly matches
the sealed snapshot. After the route writes, it checks all three routes at their
current generations, both effective 660-second backend timeouts, the generated
`/ready` health check, byte-identical app/direct kernel responses, typed
anonymous auth, both discovery `404`s, and both HTTP to HTTPS redirects. A
second effective-state read closes the verification window.

On any failure after mutation starts, the same process captures the original
exit status and then masks `HUP`, `INT`, `QUIT`, and `TERM` as its first state-changing
action, before initializing rollback state, clearing its `EXIT` trap, or
disabling fail-fast execution. It validates all four captured objects against
the in-process seal whose value was externally recorded, preflights every
current UID/spec before its first restore write, and attempts each route restore
with fresh `resourceVersion` compare-and-swap reads. It restores the legacy health policy
only after the state checker proves all three routes and their identities back
at the sealed snapshot, proves the health-policy and effective GCP paths are each
within the closed legacy transition set `{/livez,/ready}`, and reruns the exact
source-image probe. An adopted snapshot requires `/ready` for both paths. A
fresh health read then skips the health write if it already matches the sealed
snapshot; otherwise the compare-and-swap restore proceeds. This handles a failed
initial health write without inventing a `/ready` prerequisite, while refusing
to change health over unproved routes or an unknown effective path. It then
waits for the exact source specs and current route generations, proves the
restored effective health check and backends, reruns the old-image probes, and
performs a final sealed-state read. Before reporting rollback success it then
revalidates the externally sealed pre-adoption projection, takes a fresh stable
two-read closed projection of the complete Kubernetes and Compute graph, and
compares behavior derived from every retained backend-service, health-check,
URL-map, proxy, certificate, firewall, and NEG field with behavior derived from
that sealed source capture. A partial route/health predicate cannot establish
rollback success. The rollback directory is retained after any
post-mutation failure, including a successfully proved automatic rollback.
The successful adoption path also masks `HUP`, `INT`, `QUIT`, and `TERM` before it marks
the mutation closed or clears the `EXIT` rollback trap. Temporary cleanup and
the final custody path/digest output occur while those signals remain masked;
their default dispositions are restored only after that output completes.

`SIGKILL`, operator-host loss, or an unproved automatic rollback requires the
manual path below from the same clean reviewed commit. Recover the retained
four-file directory and the independently recorded digest first; if either is
lost, stop for incident recovery because the digest alone cannot reconstruct a
rollback. Manual restore has its own explicit confirmation and refuses a
different or unsealed directory. Re-establish the exclusive spec-writer window
before setting its attestation; the program validates the required source
projection directory, ownership, modes, independently supplied SHA-256, and
capture-to-projection regeneration before its first restore write. It also
requires the retained admission-bootstrap receipt and its independently
recorded digest, re-verifies the exact live policy and binding, and stops before
any restore write if that custody or enforcement proof is unavailable. Both
`capture.json` and `projection.json` must also be schema version 1, mode
`source`, and the exact pre-adoption lifecycle root
`{stage:"source",predecessorSha256:"genesis"}`; an adopted, fenced, rollback, or
final projection cannot authorize manual restore. It then
server-side dry-runs all four sealed snapshot replacements against the
then-current objects before that write and repeats the full projection proof
after convergence:

```sh
CMA_GATEWAY_RELEASE_REVISION='<reviewed-full-SHA>'
cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref \
  --points-at "${CMA_GATEWAY_RELEASE_REVISION}" \
  --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' 'refs/tags/v*')"
[[ "${cma_gateway_release_tag_record}" =~ ^(v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))\|tag\|commit\|${CMA_GATEWAY_RELEASE_REVISION}$ ]]
cma_gateway_release_tag="${BASH_REMATCH[1]}"
unset cma_gateway_release_tag_record
CMA_GATEWAY_RELEASE_REVISION="${CMA_GATEWAY_RELEASE_REVISION}" \
CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-route" \
CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer \
CMA_GATEWAY_ROLLBACK_SHA256='<externally-recorded-rollback-SHA-256>' \
CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>' \
CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>' \
CMA_GATEWAY_PROJECTION_DIR='<absolute-reviewed-source-projection-directory>' \
CMA_GATEWAY_PROJECTION_SHA256='<externally-recorded-source-projection-SHA-256>' \
CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  .github/scripts/cma-trigger-gateway-launch.sh adoption \
    --restore '<retained-sealed-rollback-directory>'
```

Do not begin the database cutover unless `--adopt` reports that the route and
`/ready` health check are effective. Keep them unchanged through the writer
drain and both recovery captures.

Apply and verify the runtime ConfigMap before allowing the new API Deployment to start. Do not apply `infra/api/` as an unordered directory: Deployment pods consume the ConfigMap through `envFrom`, and a pod that starts with the previous value will not reload a later ConfigMap update. The observed value must exactly equal the verified runtime reference copied into the deployment registry before the Deployment is applied:

```sh
kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot apply -f infra/api/configmap.yaml
kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get configmap cma-agent-manager \
  -o jsonpath='{.data.CHRONO_SANDBOX_IMAGE}{"\n"}'
# Compare the printed value with the copied, digest-verified runtime reference.
```

For an ordinary release after the first-production cutover is complete, apply the new Deployment only after that digest comparison:

```sh
kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot apply -f infra/api/deployment.yaml
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

## 5. Historical first production MongoDB cutover

This legacy session-schema migration is not the three-service shared-schema
deployment described in [deployment readiness](CMA_Deployment_Readiness.md).
The canonical service rename introduces the `cma-api` compatibility Service in
addition to `cma-agent-manager`. A single-Service fence below cannot prove a
complete writer drain for that topology: any future use requires a separately
reviewed fence that includes both Services and their callers. Do not run this
historical migration for the shared `chrono-bot` service activation.

The first production release containing these migrations needs **one continuous writer drain and two recovery units**. Capture A preserves the raw `sessions` and `session_events` collections for wholesale rollback to the old image. Capture B preserves verified, UUID-bound `sessions_v2` and `session_events_v2` before coordination can begin. Do not resume a database, object-storage, or provider writer between them.

Production must remain on `v0.7.0` until operators have proved Atlas backup/PITR or snapshot readiness and a restore path, plus version retention and a matching restore path for `CHRONO_CONTEXT_BUCKET`. Record the evidence, recovery identifiers, timestamps, retention, and restore owner before starting. A backup policy that has not produced and restored a usable recovery point is not evidence.

Run this once for each existing production database:

1. Confirm the old deployment is healthy. Verify directly that `sessions` and `session_events` are writable collections, record their UUIDs and document counts, and confirm `sessions_v2`, `sessions_v3`, `session_events_v2`, `session_coordination_receipts`, and `_cma_session_namespace_cutovers` are absent. If the source is not exactly raw legacy state, stop and investigate; do not adapt this procedure around it.
2. Fence ordinary traffic, then stop every old CMA replica and prevent it from returning. Before this first production write, establish the full checked-routing-graph writer freeze defined in section 4 and export `CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer`; keep that real freeze in force through the fence proof and scale-down. This freeze must also block every Pod create/update path that could add `release.chrono-ai.fun/cutover-fence=mongodb-715`; reserve that selector through admission policy or an equivalently enforced deployment freeze. Invoke the reviewed `--fence` transaction rather than pasting a raw Service patch. It proves before, throughout, and after the drain that no Pod carries the reserved label, compare-and-replaces the Service's exact source selector with that label, and then requires the identical two-sweep pinned Service/EndpointSlice/all-zone NEG drain proof used by restoration. The JSON Patch `test` operation makes the selector change fail closed if the live value has drifted; investigate drift instead of editing the expected value during the release. A nonzero patch result is ambiguous rather than proof of failure: the transaction re-reads the live Service and succeeds only if the exact pinned fence plus the complete external drain are independently proved. Before its Service patch, the entrypoint prints the already committed durable receipt path and SHA-256. Persist that acknowledgement from the release terminal even when a later drain or projection step fails. If an ambiguous exit leaves the exact pinned fence in place, export that independently recorded digest and rerun the reviewed `--fence` entrypoint as shown in the second block below; never derive the digest from local custody and never patch back to source. Keep this Service fence unchanged through both captures, coordination activation, and the final unflagged restart. It covers both routing generations: the legacy app-origin path reaches `Service/cma-agent-manager` through the web image's nginx proxy, while the adopted app-origin and direct API Gateway rules target that Service themselves. Only the adopted direct route may be reopened for the new API.

   Run both blocks below in one dedicated shell and keep the writer freeze in force between them:

   ```bash
   set -euo pipefail
   : "${CMA_GATEWAY_RELEASE_REVISION:?export the reviewed full release commit before fencing traffic}"
   : "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?export sole-spec-writer only after establishing the section 4 writer freeze}"
   cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref \
     --points-at "${CMA_GATEWAY_RELEASE_REVISION}" \
     --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' 'refs/tags/v*')"
   [[ "${cma_gateway_release_tag_record}" =~ ^(v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))\|tag\|commit\|${CMA_GATEWAY_RELEASE_REVISION}$ ]]
   cma_gateway_release_tag="${BASH_REMATCH[1]}"
   unset cma_gateway_release_tag_record
   CMA_GATEWAY_CONFIRM="fence-${cma_gateway_release_tag}-traffic" \
   CMA_GATEWAY_RELEASE_REVISION="${CMA_GATEWAY_RELEASE_REVISION}" \
   CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM="${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}" \
   CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>' \
   CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>' \
   CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM=live-denial-drill-v1 \
   CMA_GATEWAY_SOURCE_PROJECTION='<absolute-adopted-source-projection>/projection.json' \
   CMA_GATEWAY_SOURCE_PROJECTION_SHA256='<independently-recorded-adopted-projection-SHA-256>' \
   CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256='<independently-recorded-source-projection-SHA-256>' \
   CMA_GATEWAY_FENCE_RECEIPT_DIR='<absolute-off-host-fence-receipt-directory>' \
   CMA_GATEWAY_FENCE_RECEIPT_CONFIRM=durable-prepatch-root-v1 \
   CMA_GATEWAY_FENCED_PROJECTION_PARENT='<absolute-off-host-fenced-projection-directory>' \
   CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
   CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
     .github/scripts/cma-trigger-gateway-launch.sh traffic --fence
   ```

   The receipt acknowledgement is printed before the Service mutation and therefore remains in the durable terminal record even if the command later exits nonzero. Before any retry or scale-down, record its exact path and SHA-256 outside the operator host. Export that independently recorded digest as `CMA_GATEWAY_FENCE_RECEIPT_SHA256`; do not derive it again from the local receipt for the next proof. A successful fence also prints the durable `fenced-pre-scale` projection path and SHA-256. Record that digest independently as `CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256` before scaling down or starting the separate fence-proof process.

   The fence writer also emits `CMA fence receipt candidate before publication`
   to stderr with the intended receipt path and digest before its atomic rename.
   That candidate line is not a success acknowledgement. Retain it so the exact
   identity is still available if rename succeeds but a later directory-fsync
   barrier fails. In that recovery case, verify the published bundle at the
   candidate path, retain the candidate digest outside local custody, export it
   as `CMA_GATEWAY_FENCE_RECEIPT_SHA256`, and rerun `--fence`. The retry accepts
   and durably re-synchronizes only the byte-identical receipt it had constructed;
   never replace the externally held digest with `receipt.sha256` from the same
   local bundle.

   ```bash
   : "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:?export the independently recorded fence-receipt SHA-256}"
   : "${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:?export the independently recorded fenced-pre-scale projection SHA-256}"
   : "${CMA_GATEWAY_RELEASE_REVISION:?retain the reviewed full release commit}"
   : "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?retain the checked-routing-graph writer freeze}"
   cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref \
     --points-at "${CMA_GATEWAY_RELEASE_REVISION}" \
     --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' 'refs/tags/v*')"
   [[ "${cma_gateway_release_tag_record}" =~ ^(v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))\|tag\|commit\|${CMA_GATEWAY_RELEASE_REVISION}$ ]]
   cma_gateway_release_tag="${BASH_REMATCH[1]}"
   unset cma_gateway_release_tag_record
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot scale deployment/cma-agent-manager --replicas=0
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-agent-manager --timeout=120s
   CMA_GATEWAY_CONFIRM="prove-${cma_gateway_release_tag}-fence" \
   CMA_GATEWAY_RELEASE_REVISION="${CMA_GATEWAY_RELEASE_REVISION}" \
   CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM="${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}" \
   CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>' \
   CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>' \
   CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM=live-denial-drill-v1 \
   CMA_GATEWAY_FENCE_RECEIPT_DIR='<absolute-off-host-fence-receipt-directory>' \
   CMA_GATEWAY_FENCE_RECEIPT_SHA256="${CMA_GATEWAY_FENCE_RECEIPT_SHA256}" \
   CMA_GATEWAY_FENCED_PROJECTION_PARENT='<absolute-off-host-fenced-projection-directory>' \
   CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256}" \
   CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
   CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
     .github/scripts/cma-trigger-gateway-launch.sh traffic --prove-fence
   ```

   The first transaction refuses to return until it has re-verified the clean release checkout, applied or independently observed the exact source-to-fence compare-and-swap, and performed the pinned Service/EndpointSlice/all-zone NEG proof. Its durable `fenced-pre-scale` projection records the exact `cma-agent-manager` Deployment UID and the pre-scale Pod name/UID set rooted in the pre-patch receipt. The second transaction authenticates that predecessor against the independently retained pre-scale digest and revalidates the independently authenticated fence receipt after scale-down, requires the same Deployment UID to be stably observed at zero desired, current, updated, ready, and available replicas, requires the label-selected Pod list to be empty, and independently reads every recorded predecessor Pod name with `--ignore-not-found` to prove deletion. Only then does it repeat the complete external drain proof and commit the successor `fenced-post-scale` projection required by step 10. Record the second transaction's printed projection path and SHA-256 outside the operator host as `CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256` before step 10; the restore process will not trust a digest recovered from the colocated projection receipt. Every advertised zonal NEG must be empty across two coherent sweeps under unchanged Service UID, resourceVersion, NEG identity, and EndpointSlice identities. A missing Compute permission, malformed provider response, identity drift, nonempty NEG, undeleted predecessor Pod, incomplete Deployment convergence, or deadline failure stops the cutover without emitting the post-scale artifact and with no automatic reopen. The zero-writer boundary is reached only after this external drain proof, the Deployment is at zero, every old API pod is confirmed deleted, and every outstanding MongoDB operation, object mutation, and external provider create, delete, or replacement call has finished. Rotate or revoke the old workload's database, storage, and provider credentials where the deployment permits that stronger fence. If any later step fails, leave the no-match Service selector in place until an explicit incident decision is complete. Do not install a shell trap or other automatic cleanup that could reopen traffic after a failed release.

3. **Capture recovery unit A.** At this zero-writer boundary, take one transaction-consistent MongoDB backup or snapshot containing the raw collections and a version-retained recovery point for `CHRONO_CONTEXT_BUCKET`. Record both recovery identifiers and timestamps as one unit. If object storage is deliberately unconfigured, record that fact with the MongoDB recovery point. Prove the capture completed successfully before continuing.
4. Select the explicit legacy maintenance stage in the live ConfigMap, then apply the new API Deployment. The tuple is exact: stage `legacy`, legacy acknowledgement `drained`, and an empty coordination acknowledgement. Applying the Deployment is safe only after capture A because its first new pod now has no normal-startup path. Run this in a dedicated shell that remains open through capture B so the exact pod identity selected here remains available to step 5.

   ```bash
   set -euo pipefail
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot patch configmap cma-agent-manager --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"legacy","CHRONO_MONGODB_LEGACY_CUTOVER":"drained","CHRONO_MONGODB_COORDINATION_CUTOVER":""}}'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot apply -f infra/api/deployment.yaml
   cma_stage_pod_attempt=0
   while :; do
     cma_stage_pods_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-agent-manager -o json)"
     cma_stage_pod_count="$(printf '%s' "${cma_stage_pods_json}" | jq -er '.items | length')"
     case "${cma_stage_pod_count}" in
       0)
         cma_stage_pod_attempt=$((cma_stage_pod_attempt + 1))
         if [ "${cma_stage_pod_attempt}" -ge 60 ]; then
           echo "no cma-agent-manager stage pod appeared within two minutes; traffic remains fenced" >&2
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
         echo "more than one cma-agent-manager stage pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait \
     --for='jsonpath={.status.containerStatuses[?(@.name=="api")].started}=true' \
     "pod/${cma_stage_pod}" --timeout=31m
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_stage_pod}" -o json | \
     jq -e --arg uid "${cma_stage_pod_uid}" \
     '.metadata.uid == $uid and (.status.containerStatuses // [] | map(select(.name == "api"))) as $api | ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0'
   ```

   The creation gate waits up to two minutes for one pod and fails immediately if it observes more than one. Do not use `kubectl rollout status` for this stage. The Deployment cannot become Available because its sole pod is deliberately never Ready. The command instead waits up to 31 minutes for the exact captured `api` pod to pass its 30-minute startup probe, then rejects replacement or restart. Liveness and readiness remain suppressed until the post-shutdown `/livez` listener binds. A timeout or restart means the boundary was not reached and must be investigated with the Service fence still in place, not treated as capture B.

5. Verify the maintenance boundary before capture B. Recheck the exact API pod captured in step 4 and port-forward directly to it in that dedicated terminal. This bypasses the intentionally fenced Service. Leave the port-forward running while performing the HTTP checks, then stop it manually; its exit, replacement, or restart is a failed verification, not permission to continue.

   ```bash
   set -euo pipefail
   : "${cma_stage_pod:?step 4 did not capture the stage pod}"
   : "${cma_stage_pod_uid:?step 4 did not capture the stage pod UID}"
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_stage_pod}" -o json | \
     jq -e --arg uid "${cma_stage_pod_uid}" \
     '.metadata.uid == $uid and (.status.containerStatuses // [] | map(select(.name == "api"))) as $api | ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot port-forward \
     "pod/${cma_stage_pod}" 18080:8080
   ```

   From another terminal, mechanically assert the exact maintenance responses. The live route must return body `ok` with `200`. `/ready` and the real session collection route must return the exact staged reason with `503`, while `HEAD /livez` must also return `503`.

   ```bash
   (
     set -euo pipefail
     cma_stage_unavailable='{"status":"not-ready","reason":"legacy-cutover-staged"}|503'
     cma_stage_livez="$(curl --disable --noproxy '*' --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/livez)"
     test "${cma_stage_livez}" = 'ok|200'
     cma_stage_ready="$(curl --disable --noproxy '*' --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/ready)"
     test "${cma_stage_ready}" = "${cma_stage_unavailable}"
     cma_stage_sessions="$(curl --disable --noproxy '*' --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/api/v1/sessions)"
     test "${cma_stage_sessions}" = "${cma_stage_unavailable}"
     cma_stage_head_livez="$(curl --disable --noproxy '*' --head --show-error --silent --max-time 10 --output /dev/null --write-out '%{http_code}' \
       http://127.0.0.1:18080/livez)"
     test "${cma_stage_head_livez}" = '503'
     kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get service cma-agent-manager -o json | \
       jq -e '.spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}'
     kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-agent-manager -o json | \
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
   set -euo pipefail
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot patch configmap cma-agent-manager --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"","CHRONO_MONGODB_LEGACY_CUTOVER":"","CHRONO_MONGODB_COORDINATION_CUTOVER":"drained"}}'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot scale deployment/cma-agent-manager --replicas=0
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-agent-manager --timeout=120s
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot scale deployment/cma-agent-manager --replicas=1
   cma_activation_pod_attempt=0
   while :; do
     cma_activation_pods_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-agent-manager -o json)"
     cma_activation_pod_count="$(printf '%s' "${cma_activation_pods_json}" | jq -er '.items | length')"
     case "${cma_activation_pod_count}" in
       0)
         cma_activation_pod_attempt=$((cma_activation_pod_attempt + 1))
         if [ "${cma_activation_pod_attempt}" -ge 60 ]; then
           echo "no cma-agent-manager activation pod appeared within two minutes; traffic remains fenced" >&2
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
         echo "more than one cma-agent-manager activation pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait \
     --for=condition=Ready "pod/${cma_activation_pod}" --timeout=31m
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_activation_pod}" -o json | \
     jq -e --arg uid "${cma_activation_pod_uid}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   ```

   The bounded creation gate refuses multiple candidates, and the 31-minute wait covers the manifest's 30-minute startup-probe budget for only the captured pod. A timeout, replacement, or restart does not cancel or roll back database work: leave the Service fenced and every old writer stopped while investigating the candidate.

8. Recheck the exact Ready activation pod captured in step 7 and port-forward directly to it. Reject replacement or a startup restart before accepting the candidate.

   ```sh
   : "${cma_activation_pod:?step 7 did not capture the activation pod}"
   : "${cma_activation_pod_uid:?step 7 did not capture the activation pod UID}"
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_activation_pod}" -o json | \
     jq -e --arg uid "${cma_activation_pod_uid}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot port-forward \
     "pod/${cma_activation_pod}" 18080:8080
   ```

   Through that direct connection, run this exact normal-mode HTTP and fence gate from another terminal:

   ```bash
   (
     set -euo pipefail
     cma_activation_livez="$(curl --disable --noproxy '*' --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/livez)"
     test "${cma_activation_livez}" = 'ok|200'
     cma_activation_ready="$(curl --disable --noproxy '*' --fail --show-error --silent --max-time 10 --write-out '|%{http_code}' \
       http://127.0.0.1:18080/ready)"
     test "${cma_activation_ready}" = '{"status":"ready"}|200'
     kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get service cma-agent-manager -o json | \
       jq -e '.spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}'
     kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get endpointslice \
       -l kubernetes.io/service-name=cma-agent-manager -o json | \
       jq -e '[.items[].endpoints[]? | select(.conditions.ready != false)] | length == 0'
   )
   ```

   Perform the representative authenticated reads through that same port-forward; do not probe through the fenced Service. Verify directly that `sessions` views `sessions_v2`, `sessions_v2` views the sole writable `sessions_v3`, and the original `sessions_v2` UUID moved with the rename to `sessions_v3`. Confirm the strict session validator admits only `_cma_write_protocol: 2` with `_cma_coordination_protocol: 3`, every session carries both exact values, receipts carry write protocol 1 with coordination protocol 2, both coordination provenance records are complete and UUID-bound, and preserved history/custody/context/write/tombstone state reconciles with capture B. Stop the port-forward manually after verification. Any failure leaves traffic fenced.

9. Perform the final unflagged restart while ordinary traffic remains disabled. Clear the coordination acknowledgement, delete the flagged pod completely, start one replacement, and verify that its creation follows the ConfigMap change. Run the whole step in a dedicated shell that remains open through step 10; the pod identity captured before the Ready wait is the only candidate that may later receive traffic.

   ```bash
   set -euo pipefail
   : "${CMA_EXPECTED_API_IMAGE_ID:?set the immutable registry imageID from the verified release-image evidence}"
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot patch configmap cma-agent-manager --type merge \
     -p '{"data":{"CHRONO_MONGODB_CUTOVER_STAGE":"","CHRONO_MONGODB_LEGACY_CUTOVER":"","CHRONO_MONGODB_COORDINATION_CUTOVER":""}}'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot scale deployment/cma-agent-manager --replicas=0
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait --for=delete pod \
     -l app.kubernetes.io/name=cma-agent-manager --timeout=120s
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot scale deployment/cma-agent-manager --replicas=1
   cma_final_pod_attempt=0
   while :; do
     cma_final_pods_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod \
       -l app.kubernetes.io/name=cma-agent-manager -o json)"
     cma_final_pod_count="$(printf '%s' "${cma_final_pods_json}" | jq -er '.items | length')"
     case "${cma_final_pod_count}" in
       0)
         cma_final_pod_attempt=$((cma_final_pod_attempt + 1))
         if [ "${cma_final_pod_attempt}" -ge 60 ]; then
           echo "no final cma-agent-manager pod appeared within two minutes; traffic remains fenced" >&2
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
         echo "more than one final cma-agent-manager pod exists; traffic remains fenced" >&2
         exit 1
         ;;
     esac
   done
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot wait \
     --for=condition=Ready "pod/${cma_final_pod}" --timeout=31m
   cma_final_pod_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_final_pod}" -o json)"
   cma_final_pod_ip="$(printf '%s' "${cma_final_pod_json}" | jq -er '.status.podIP')"
   cma_final_pod_image="$(printf '%s' "${cma_final_pod_json}" | jq -er \
     'if (.spec.containers | length) == 1 and .spec.containers[0].name == "api" then .spec.containers[0].image else error("final Pod container set is not exact") end')"
   cma_final_pod_image_id="$(printf '%s' "${cma_final_pod_json}" | jq -er \
     'if (.status.containerStatuses | length) == 1 and .status.containerStatuses[0].name == "api" then .status.containerStatuses[0].imageID else error("final Pod status set is not exact") end')"
   cma_final_replica_set="$(printf '%s' "${cma_final_pod_json}" | jq -er \
     'if .metadata.ownerReferences == [(.metadata.ownerReferences[0])] and .metadata.ownerReferences[0].apiVersion == "apps/v1" and .metadata.ownerReferences[0].kind == "ReplicaSet" and .metadata.ownerReferences[0].controller == true and .metadata.ownerReferences[0].blockOwnerDeletion == true then .metadata.ownerReferences[0].name else error("final Pod owner is not one exact ReplicaSet") end')"
   cma_final_replica_set_uid="$(printf '%s' "${cma_final_pod_json}" | jq -er \
     '.metadata.ownerReferences[0].uid')"
   cma_final_replica_set_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get replicaset "${cma_final_replica_set}" -o json)"
   cma_final_deployment_uid="$(printf '%s' "${cma_final_replica_set_json}" | jq -er \
     --arg uid "${cma_final_replica_set_uid}" \
     'if .metadata.uid == $uid and .metadata.ownerReferences == [(.metadata.ownerReferences[0])] and .metadata.ownerReferences[0].apiVersion == "apps/v1" and .metadata.ownerReferences[0].kind == "Deployment" and .metadata.ownerReferences[0].name == "cma-agent-manager" and .metadata.ownerReferences[0].controller == true and .metadata.ownerReferences[0].blockOwnerDeletion == true then .metadata.ownerReferences[0].uid else error("final ReplicaSet owner is not the exact cma-agent-manager Deployment") end')"
   cma_final_deployment_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get deployment cma-agent-manager -o json)"
   test "${cma_final_pod_image_id}" = "${CMA_EXPECTED_API_IMAGE_ID}"
   [[ "${cma_final_pod_image_id}" =~ ^(docker-pullable://)?us-central1-docker\.pkg\.dev/example-project-01/example-registry-02/cma-api@sha256:[0-9a-f]{64}$ ]]
   printf '%s' "${cma_final_pod_json}" | \
     jq -e --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" \
       --arg rs "${cma_final_replica_set}" --arg rs_uid "${cma_final_replica_set_uid}" \
       --arg image "${cma_final_pod_image}" --arg image_id "${CMA_EXPECTED_API_IMAGE_ID}" \
     '.metadata.uid == $uid and .status.podIP == $ip and .metadata.ownerReferences == [{apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,kind:"ReplicaSet",name:$rs,uid:$rs_uid}] and (.status.containerStatuses // [] | map(select(.name == "api" and .image == $image and .imageID == $image_id and .ready == true and .started == true and .restartCount == 0)) | length) == 1 and (.status.conditions // [] | map(select(.type == "Ready" and .status == "True")) | length) == 1'
   printf '%s' "${cma_final_replica_set_json}" | jq -e \
     --arg uid "${cma_final_replica_set_uid}" --arg deployment_uid "${cma_final_deployment_uid}" \
     '.metadata.uid == $uid and .metadata.ownerReferences == [{apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,kind:"Deployment",name:"cma-agent-manager",uid:$deployment_uid}] and .spec.replicas == 1 and .status.observedGeneration == .metadata.generation and .status.replicas == 1 and .status.readyReplicas == 1 and .status.availableReplicas == 1'
   printf '%s' "${cma_final_deployment_json}" | jq -e \
     --arg uid "${cma_final_deployment_uid}" \
     '.metadata.uid == $uid and .metadata.name == "cma-agent-manager" and .metadata.namespace == "chrono-bot" and .spec.replicas == 1 and .status.observedGeneration == .metadata.generation and .status.replicas == 1 and .status.updatedReplicas == 1 and .status.readyReplicas == 1 and .status.availableReplicas == 1 and (.status.unavailableReplicas // 0) == 0'
   (
     cma_workload_proof_dir="$(mktemp -d)"
     trap 'rm -rf "${cma_workload_proof_dir}"' EXIT HUP INT QUIT TERM
     chmod 700 "${cma_workload_proof_dir}"
     printf '%s\n' "${cma_final_deployment_json}" >"${cma_workload_proof_dir}/deployment.json"
     printf '%s\n' "${cma_final_replica_set_json}" >"${cma_workload_proof_dir}/replicaset.json"
     printf '%s\n' "${cma_final_pod_json}" >"${cma_workload_proof_dir}/pod.json"
     chmod 600 "${cma_workload_proof_dir}"/*.json
     ruby --disable-gems .github/scripts/cma-trigger-gateway-projection.rb \
       --validate-workload infra/api/deployment.yaml \
       "${cma_workload_proof_dir}/deployment.json" \
       "${cma_workload_proof_dir}/replicaset.json" \
       "${cma_workload_proof_dir}/pod.json"
   )
   ```

   The bounded creation gate fails immediately on multiple candidates. A timeout, replacement, or restart again leaves traffic fenced and does not prove that database work stopped. Port-forward directly to the already captured and verified pod from the same shell:

   ```sh
   : "${cma_final_pod:?step 9 did not capture the verified final pod}"
   : "${cma_final_pod_uid:?step 9 did not capture the verified final pod UID}"
   : "${cma_final_pod_ip:?step 9 did not capture the verified final pod IP}"
   cma_final_pod_json="$(kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot get pod "${cma_final_pod}" -o json)"
   printf '%s' "${cma_final_pod_json}" | \
     jq -e --arg uid "${cma_final_pod_uid}" --arg ip "${cma_final_pod_ip}" \
     '(.status.containerStatuses // [] | map(select(.name == "api"))) as $api | (.status.conditions // [] | map(select(.type == "Ready" and .status == "True"))) as $ready | .metadata.uid == $uid and .status.podIP == $ip and ($api | length) == 1 and $api[0].started == true and $api[0].restartCount == 0 and ($ready | length) == 1'
   kubectl --context gke_example-project-02_asia-northeast1_example-cluster-02 --namespace chrono-bot port-forward \
     "pod/${cma_final_pod}" 18080:8080
   ```

   Verify all three live cutover values are empty and the same protected UUIDs and contracts remain. In the other terminal, rerun the exact normal-mode HTTP and fence gate from step 8 and perform the representative authenticated reads through the port-forward. Stop the port-forward manually but keep this shell and every captured `cma_final_*` workload identity and image variable before restoring traffic.

10. Restore ordinary traffic only after every final check has passed. In the retained step 9 parent shell, disable xtrace, establish `umask 077`, and require `ulimit -c 0` before retrieving or exporting the short-lived bearer; abort while traffic remains fenced if any protection cannot be established. Re-establish the full checked-routing-graph freeze from the adoption section, derive the distinct destructive-intent guard as `restore-<exact-release-tag>-traffic` from the single annotated SemVer tag on `CMA_GATEWAY_RELEASE_REVISION`, and export `CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer`; keep that window through the final post-open route/policy proof, success output, and re-fence trap removal. Only after the parent-shell protections are active, export a short-lived release access token as `CHRONO_RELEASE_ACCESS_TOKEN`, the exact reviewed 40-hex commit as `CMA_GATEWAY_RELEASE_REVISION`, the retained adoption snapshot path as `CMA_GATEWAY_ROLLBACK_DIR`, and its independently recorded digest as `CMA_GATEWAY_ROLLBACK_SHA256` in that same shell. Invoke the checked-in program only through `cma-trigger-gateway-launch.sh` from the clean checkout root; do not paste, source, or execute the traffic implementation directly. At process startup, before parsing the remaining program or reading the inherited bearer, the launcher and traffic program independently re-establish `umask 077`, disable core dumps, and disable inherited xtrace. Before its first cluster action, the program verifies the tag-derived confirmation, exact freeze attestation, clean reviewed commit, and its own executable Git blob. On every pre-validation recheck it also byte-binds the Gateway state checker, sourced adoption helper, API/web Deployment manifests, and API ConfigMap to their exact reviewed Git blobs, then revalidates the external snapshot seal before invoking the checker. The checker binds the adopted live objects' UIDs and owned metadata to that snapshot. Every Compute truth read starts `gcloud` under `env -i`, retaining only the operator's `HOME` for the default Cloud SDK active configuration, the current command `PATH`, `LC_ALL=C`, and `CLOUDSDK_CORE_HTTP_TIMEOUT=15`; inherited SDK property, endpoint, credential, custom-CA, Python, and proxy variables cannot redirect those reads. The launcher removes the credential from the exported environment before spawning any child and supplies it to the traffic program through protected descriptor 8; only `curl` receives it through a separate protected descriptor rather than an argument. Every Kubernetes and Compute call has both a client request timeout and an outer process deadline, the checker has its own outer deadline, and each EndpointSlice/NEG convergence operation has an explicit five-minute `SECONDS` deadline. Replace the exact fence selector with the source selector by another compare-and-replace JSON Patch, wait for coherent EndpointSlice and zonal NEG convergence, require exactly the verified Deployment-to-ReplicaSet-to-Pod chain and immutable image digest in both projections, and verify authenticated reads through both the direct API hostname and the app-origin API route. The installed traps only re-fence traffic; they never reopen it. The program erases its private credential copy on success, and the calling shell clears its copy after either success or failure.

   The retained shell must also pass the durable admission receipt and its independently recorded digest, the live-denial drill acknowledgement, the original fence-receipt custody directory and independently recorded receipt digest, the fenced projection parent used before scale-down, the independently recorded `fenced-pre-scale` and `fenced-post-scale` projection digests, and a pre-existing owned final-projection parent. Every successor validator receives and enforces the exact expected lifecycle stage and predecessor digest again after capture publication. These are inputs to the pre-reopen and final closed projections; none is reconstructed from a colocated checksum or transient shell state.

The checked adoption and traffic-restoration executables are intentionally
hard-disabled by a reviewed source constant in this evidence-tooling revision.
Neither ordinary confirmation nor a passing partial checker can bypass it.
Change that constant only in the reviewed commit that contains the closed
Compute projection, origin/public-chain probes, selected client-IP authority
implementation, and their complete evidence fixtures. Restoration also
removes inherited TLS trust/backend/key-log overrides, ignores curl
configuration and inherited proxies, rejects CR or LF in the bearer before
constructing its private header, and fail-closed establishes `umask 077` and
disables core dumps at process startup, before the inherited bearer can be
handled or function definitions parsed. It also rejects a convergence proof that crosses its wall-clock deadline
even when the last bounded provider call returned a matching object.

The first coherent two-sweep fence proof pins the `cma-agent-manager` Service UID and
its exact NEG name and zone set for the whole transaction. Selector reads,
every re-fence, both reopen proofs, and the final projection must retain that
root; deleting and recreating an exact-looking Service is a failure even when
its new `resourceVersion`, selector, annotations, and endpoints converge.

```bash
# Run this in the retained step 9 shell before retrieving or exporting the bearer.
set +x
umask 077
if ! ulimit -c 0 || [ "$(ulimit -c)" != 0 ]; then
  echo "core dumps could not be disabled; traffic remains fenced" >&2
  exit 1
fi

# Retrieve and export the short-lived bearer only after the checks above.
: "${CHRONO_RELEASE_ACCESS_TOKEN:?export the short-lived release access token while traffic is fenced}"
: "${CMA_GATEWAY_RELEASE_REVISION:?export the reviewed full release commit while traffic is fenced}"
: "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?re-establish the checked-routing-graph freeze while traffic is fenced}"
: "${CMA_GATEWAY_ROLLBACK_DIR:?export the retained sealed adoption snapshot path}"
: "${CMA_GATEWAY_ROLLBACK_SHA256:?export the independently recorded adoption snapshot digest}"
: "${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:?export the independently recorded fenced-pre-scale projection SHA-256}"
: "${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256:?export the independently recorded fenced-post-scale projection SHA-256}"
cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref \
  --points-at "${CMA_GATEWAY_RELEASE_REVISION}" \
  --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' 'refs/tags/v*')"
[[ "${cma_gateway_release_tag_record}" =~ ^(v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))\|tag\|commit\|${CMA_GATEWAY_RELEASE_REVISION}$ ]]
cma_gateway_release_tag="${BASH_REMATCH[1]}"
unset cma_gateway_release_tag_record
cma_restore_status=0
# shellcheck disable=SC2154 # Retained variables are set in this step 9 parent shell.
CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-traffic" \
CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>' \
CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>' \
CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM=live-denial-drill-v1 \
CMA_GATEWAY_FENCE_RECEIPT_DIR='<absolute-off-host-fence-receipt-directory>' \
CMA_GATEWAY_FENCE_RECEIPT_SHA256='<externally-recorded-fence-receipt-SHA-256>' \
CMA_GATEWAY_FENCED_PROJECTION_PARENT='<absolute-off-host-fenced-projection-directory>' \
CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256}" \
CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256}" \
CMA_GATEWAY_FINAL_PROJECTION_PARENT='<absolute-off-host-final-projection-directory>' \
CMA_FINAL_DEPLOYMENT_UID="${cma_final_deployment_uid}" \
CMA_FINAL_REPLICA_SET="${cma_final_replica_set}" \
CMA_FINAL_REPLICA_SET_UID="${cma_final_replica_set_uid}" \
CMA_FINAL_POD="${cma_final_pod}" \
CMA_FINAL_POD_UID="${cma_final_pod_uid}" \
CMA_FINAL_POD_IP="${cma_final_pod_ip}" \
CMA_FINAL_POD_IMAGE="${cma_final_pod_image}" \
CMA_FINAL_POD_IMAGE_ID="${cma_final_pod_image_id}" \
CMA_GATEWAY_TRUSTED_PATH='<reviewed-absolute-command-search-path>' \
CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  .github/scripts/cma-trigger-gateway-launch.sh traffic || \
    cma_restore_status=$?
unset CHRONO_RELEASE_ACCESS_TOKEN
if [ "${cma_restore_status}" -ne 0 ]; then
  exit "${cma_restore_status}"
fi
unset cma_restore_status
```

A nonzero response from the reopen patch is never interpreted as proof that traffic stayed fenced. The script reads the live selector: an observed source selector is immediately changed back with an exact source-to-fence patch, while an unreadable or unknown selector is a critical manual-fence condition. Zero routable EndpointSlice entries alone are not accepted as an external fence. Every initial or repeated fence proof parses the exact `cma-agent-manager` NEG name and unique zones from the fenced Service, bounded-reads every advertised zonal membership with `compute.networkEndpointGroups.listNetworkEndpoints`, and requires every response to be an empty JSON array. It re-reads and binds the Service UID, `resourceVersion`, selector, port, NEG name and zones plus the complete EndpointSlice name/UID/resourceVersion set around each Compute sweep; any change makes the proof fail rather than mixing observations. Two consecutive all-empty sweeps under the same Service and EndpointSlice records are required. Only then may re-fencing report success or set `cma_traffic_open=0`.

Reopening uses the converse proof. The only routable EndpointSlice entry must have the exact one-element address array for the verified pod plus its Pod name, namespace, and UID. Across all advertised zones, the NEG population must contain exactly one endpoint with that same pod IP and port `8080`, under unchanged Service and EndpointSlice records. Before the selector changes, the program also proves the retained Pod has exactly one controlling ReplicaSet owner, that ReplicaSet has exactly one controlling `cma-agent-manager` Deployment owner, and all three UIDs equal the identities captured in step 9. The reviewed projection contract parses `infra/api/deployment.yaml`, applies only the closed Kubernetes defaults named by that contract, and then requires complete equality with the live Deployment spec and ReplicaSet template. It derives the Pod spec from that same template and permits only the exact scheduler fields, default NoExecute tolerations, and one standard `kube-api-access-*` projected service-account volume/mount used by this cluster. Command, args, environment, `envFrom`, service account, security context, probes, resources, affinity, volumes, mounts, container count, or any other same-image admission/spec drift therefore fails. The runtime container status image must equal the manifest image and its `imageID` must equal the externally verified immutable registry digest captured in step 9. Both the `fenced-pre-reopen` and `final-reopen` projections capture the full normalized Deployment, retained ReplicaSet, and retained Pod objects and seal their exact ownership chain, resource identities, spec SHA-256 values, Pod IP, manifest image, and immutable `imageID` in `graph.apiWorkload.verified`. Bash binds that sealed proof back to every retained step 9 value and reruns the full reviewed workload validator on the sealed objects. Each fenced and final-source validator returns the SHA-256 of the exact projection bytes that passed its lifecycle-ancestry, workload, and live-routing checks; every caller validates and acknowledges that returned digest without reopening the mutable projection path. An exact live workload recheck immediately before the Service reopen and another immediately after final projection validation bracket both capture windows, so a replacement workload cannot be transiently served or accepted as final success. The first successful endpoint call retains one normalized record containing the Service UID/resourceVersion/NEG identity, complete EndpointSlice identity and ready-endpoint projection, and full normalized NEG member object. The proof runs once before public traffic checks and again after the final route/policy check; the second result must be byte-identical to the first successful normalized Service, EndpointSlice, and NEG projection. Any extra address, workload-owner drift, workload-spec drift, image drift, Pod identity, NEG member, replacement UID, or intervening resource version triggers re-fencing. After that second proof, the program re-reads the complete Deployment-to-ReplicaSet-to-Pod chain, full workload specs, immutable imageID, Pod IP, readiness, started state, and zero restart count immediately before commit; deletion, restart, ownership drift, spec drift, or readiness drift also re-fences.

Before reopening and again after the public checks, the script reasserts the clean reviewed checkout, its own reviewed executable blob, both reviewed helper blobs, the externally authenticated snapshot, all three generation-matched routes and the health policy with snapshot-bound UIDs and owned metadata, both effective 660-second backend services bound to the advertised NEGs, and the effective `/ready` health check; drift leaves or returns traffic fenced. After reopening, a command error, an unexpected routable endpoint, a timeout, a signal, or any public or authenticated verification failure takes the same fail-closed path. The `EXIT` trap passes the already captured status into its handler; the handler masks ordinary termination signals as its first command, closes the reserved credential descriptor, and only then reads that argument or changes any other state, so a repeated signal cannot interrupt re-fencing. The discovery check parses the CMA OpenAPI 3.1 document rather than accepting an arbitrary 2xx response from the legacy SPA fallback. Only the exact pod name, UID, and address recorded in step 9 can pass the EndpointSlice gate. After both authenticated reads and the final route/policy recheck succeed, the success path masks `HUP`, `INT`, `QUIT`, and `TERM`, erases the private bearer value and captured response bodies, and prints explicit success while the `EXIT` re-fence trap remains armed. An output failure therefore re-fences. Only after that acknowledgement succeeds does it mark restoration committed, disarm the `EXIT` trap, and restore the ordinary signal dispositions.

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

**The release pull request reads `BEHIND` and will not merge.** `develop` is missing the merge commit the _previous_ release left on `main`. It carries no content — `git diff develop..main` is empty — but a merge commit is still a commit, and a protected branch requiring up-to-date branches will not take a pull request without it.

Since `v0.4.0` the publish workflow pushes a release-specific branch and opens that back-merge itself, as its last step, so this should not recur. If repository policy refuses Actions permission to open pull requests, the job fails and leaves the branch in place rather than reporting a false success. Open the branch against `develop` manually, then rerun the failed job; it recognizes the open pull request and completes without creating a duplicate. If the release gate later says `develop contains main` failed, merge that back-merge pull request. Merging `origin/main` into `develop` by hand does the same thing; do not force push, `develop` is protected.

A back-merge that **conflicts** is left open deliberately rather than resolved automatically: a conflict means `develop` moved during the release, and a resolution nobody chose is how a release silently reverts something.

**The push to GHCR is refused.** Two causes, in order of likelihood. The repository's default workflow permission is read-only — the explicit `permissions:` blocks should cover it, but confirm under Settings → Actions → General. Failing that, the organisation restricts package creation by Actions, which is not something this repository can change.

**Two merges landed together.** They do not race with each other; the workflow
serializes on a single concurrency group, and the second computes its version
after the first has tagged. This serialization does not cover an external
package writer, which is why the exclusive-writer prerequisite is mandatory.

**Three or more landed inside one build window.** A concurrency group holds one _pending_ run, not a queue, so a third merge supersedes the second's pending run and that merge is marked cancelled having published nothing.

The code is not lost: the run that does publish is building a later commit on `main`, which contains it. What is lost is that merge's notes and its declared bump — so a `release:major` can be absorbed into a following `release:patch`, and the version that ships understates what is in it.

Re-run the cancelled run from the Actions tab before the next one tags and it publishes normally. After that, its number is no longer the next one, so fold its notes into the release that did go out instead. Rare by construction, since every merge into `main` is a release — but the only sign is a cancelled run rather than a failed one, which is easy to read as noise.
