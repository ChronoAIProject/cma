# Release

A release is a merge into `main`. There is no separate step before it and none after it: the merge decides the version, publishes the images and writes the record.

This document is the procedure. The rules it implements are in [CLAUDE.md](../CLAUDE.md) §6.

---

## 1. What a release produces

| Artifact | Where | Immutable |
|---|---|---|
| `ghcr.io/chronoaiproject/cma-api:vX.Y.Z` | GHCR, private | yes |
| `ghcr.io/chronoaiproject/cma-web:vX.Y.Z` | GHCR, private | yes |
| `ghcr.io/chronoaiproject/cma-runtime:vX.Y.Z` | GHCR, private | yes |
| the same three at `:<short-sha>` | GHCR, private | yes |
| annotated tag `vX.Y.Z` | this repository | yes |
| GitHub release `vX.Y.Z` | this repository | notes editable |

The release page carries three things:

1. **The release notes**, under a `## Release Notes vX.Y.Z` heading the publisher writes, followed by the three subsections exactly as the pull request declared them.
2. **The source archives** — `Source code (zip)` and `Source code (tar.gz)`. GitHub generates these from the tag and attaches them to every release, so nothing uploads them and there is no second copy to go stale.
3. **The images** — every reference in a block you can copy, and a link to each package in GHCR.

No tag is ever moved and no tag is ever reused. There is deliberately no `latest`: a moving tag is something a manifest can be pointed at by accident, and [CLAUDE.md](../CLAUDE.md) §5 forbids a session sitting on an image whose contents can change underneath it.

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
gh api -X PATCH repos/ChronoAIProject/cma/branches/main/protection/required_status_checks \
  -F strict=true -f 'contexts[]=Release declaration' ...
```

`Images (runtime)` is deliberately absent, matching `develop`. `ci.yml` builds it and the publisher will not release without it, so it is covered — but it is not a required check, and adding it is its own change.

Then, on merge, `publish.yml`:

- resolves the pull request the merge commit came from, re-reads its label, and re-extracts its `## Release Notes` section — checked again rather than trusted, because an administrator can merge past a failing check;
- computes the next version from the highest `vX.Y.Z` tag, seeding `v0.0.0` when there is none, so the first-ever release is `v0.0.1`, `v0.1.0` or `v1.0.0` depending on the label;
- builds each image, scans it, and pushes it only if the scan passes;
- creates the tag and the release last.

Watch it under **Actions → Publish release**. It takes roughly as long as the image job on the pull request did, because it restores the same layer cache.

---

## 3. Fetching what was released

The packages are private, so authenticate first. It must be a **classic** personal access token with the `read:packages` scope — GitHub Packages supports no other kind.

A fine-grained token fails in a way worth knowing about, because nothing in it points at the token:

```
docker login ghcr.io   Login Succeeded
docker pull ...:v0.1.0  403 Forbidden ... /v2/chronoaiproject/chrono-bot-api/manifests/v0.1.0
```

The login is accepted, so the credential looks good and the image looks like the thing that is wrong — missing, misnamed, or not visible to you. It is none of those. Check the token type before checking anything else.

```sh
echo "$GITHUB_TOKEN" | docker login ghcr.io -u <your-github-username> --password-stdin
docker pull ghcr.io/chronoaiproject/cma-api:v0.1.0
```

Inside Actions this does not arise — the workflow authenticates with the job's own `GITHUB_TOKEN`, which carries `packages: write` for this repository and needs no personal token at all.

The release body lists all three references for that version, which is the quickest way to get them exactly right.

To deploy into a cluster that cannot authenticate to GHCR, pull and re-push into the registry it does use. The published images carry no provenance attestation precisely so that this is a plain copy:

```sh
docker pull  ghcr.io/chronoaiproject/cma-api:v0.1.0
docker tag   ghcr.io/chronoaiproject/cma-api:v0.1.0 <target-registry>/cma-api:v0.1.0
docker push  <target-registry>/cma-api:v0.1.0
```

Only `web` is published for arm64 as well as amd64. `api` and `runtime` are amd64 only: emulated Rust release builds ran for hours in CI, and every node in the target cluster is amd64.

---

## 4. What a release deliberately does not do

**It does not deploy, and it does not touch `infra/`.** The manifests keep naming whatever image they already name. A release makes a version available; someone still has to decide to run it.

**It especially does not move `CHRONO_SANDBOX_IMAGE`.** Under [CLAUDE.md](../CLAUDE.md) §5 that value *is* the runtime version, and every session running a different one is rotated onto it — transcript snapshotted, sandbox replaced, agent resumed. Editing it is a rollout of every live session, which is why publishing a runtime image and adopting it are separate acts. Adopt one by editing `infra/api/configmap.yaml` in its own pull request, deliberately, when a rotation is what you want.

Changing an image reference in `infra/` is a deployment-surface change and belongs in the same pull request as anything else that changes with it (§4).

---

## 5. One-time MongoDB session namespace cutover

The release that introduces `sessions_v2` and `session_events_v2` cannot be rolled out alongside a CMA version that still mutates `sessions` and `session_events`. MongoDB validators can reject old inserts and replacements, but they do not fence deletes. `CHRONO_MONGODB_LEGACY_CUTOVER=drained` is therefore an operator assertion, not a condition CMA can discover or prove.

Run this once for a database whose legacy names are still writable collections:

1. Establish a recovery baseline: take the deployment's normal MongoDB backup or snapshot, record the current collection types for `sessions` and `session_events`, and confirm the old deployment is healthy before stopping it.
2. Stop every CMA replica that can run the legacy writer. Wait for all pods/processes to terminate and for their outstanding MongoDB operations to finish. Do not start the replacement while any old replica can still receive work.
3. Set `CHRONO_MONGODB_LEGACY_CUTOVER` to the exact value `drained`.
4. Start exactly one new CMA replica and let startup finish. Do not overlap this step with an old binary or start a second migration candidate.
5. Verify directly in MongoDB that `sessions` is a read-only view of `sessions_v2` and `session_events` is a read-only view of `session_events_v2`, then verify that the replacement's `/ready` endpoint returns `200`.
6. Leave that verified migration replica running and unset `CHRONO_MONGODB_LEGACY_CUTOVER` in the deployment configuration (the empty value in `infra/api/configmap.yaml`). Do not restart the replica merely to reread a flag that a completed pair of compatibility views no longer needs.
7. Start the remaining and all future replicas without the acknowledgement. Resume the ordinary rollout or replica count only after an unflagged replica is Ready.

Default startup refuses a writable or unproven partial legacy state before renaming, dropping, or otherwise mutating either legacy namespace. Before the first action on a genuinely empty database, or after the explicit drain acknowledgement, CMA records durable in-progress provenance so a crash can resume that exact migration. Once both views are installed, it majority-writes completion with the authoritative collection UUIDs. Later startup refuses a missing or replacement namespace instead of treating post-completion data loss as a fresh database. A generic partial state without that provenance is not inferred to be safe.

---

## 6. One-time MongoDB coordination activation

The release that makes runtime authority and cleanup debt durable moves the writable session collection from `sessions_v2` to `sessions_v3`. It leaves `session_events_v2` in place. This activation cannot overlap any pre-coordination CMA process: an old process can make an external provider call before its database write, and no MongoDB validator can recall that call. `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` is the operator's assertion that both database work and provider work from those processes have permanently stopped.

Run this once for every existing CMA database, after the namespace cutover in section 5 is complete:

1. Take one transaction-consistent backup or snapshot containing `sessions_v3` if it already exists, otherwise `sessions_v2`; `session_events_v2`; `session_coordination_receipts`; `_cma_session_namespace_cutovers`; `session_writes`; and `session_tombstones`. Keep these collections and provenance records as one recovery unit. Do not combine session data from one point in time with receipts or markers from another.
2. Stop every pre-coordination CMA binary. Wait for every pod/process, outstanding MongoDB operation, and external provider create, delete, or replacement operation to finish. Prevent the old workload from returning. Rotate or revoke its database and provider credentials where the deployment permits that stronger fence.
3. Set `CHRONO_MONGODB_COORDINATION_CUTOVER` to the exact value `drained`. If section 5 has not yet run, set its separate legacy acknowledgement only after satisfying that section's drain conditions too.
4. Start exactly one new CMA replica. Do not start another activation candidate and do not restore any old replica while startup is in progress. A crash after the durable in-progress marker may be resumed by the same new binary without reasserting the flag.
5. Verify directly in MongoDB that `sessions` is a read-only view of `sessions_v2`, `sessions_v2` is a read-only view of `sessions_v3`, and `sessions_v3` is the sole writable session collection. Confirm that the coordination activation and receipt provenance records in `_cma_session_namespace_cutovers` are complete and UUID-bound, then confirm `/ready` returns `200`.
6. Leave the verified replica running and unset `CHRONO_MONGODB_COORDINATION_CUTOVER`. Start the remaining new replicas without the acknowledgement only after an unflagged replica becomes Ready.

A genuinely empty database activates automatically. Any database that existed before startup requires the exact acknowledgement, even when its session collections happen to be empty. The activation majority-writes intent before fencing and renaming `sessions_v2`; the rename preserves its collection UUID. It then creates `sessions_v2` as a view of `sessions_v3`, establishes and audits the receipt collection, and completes a marker bound to both protected UUIDs. A completed marker authorizes verification only: startup will not recreate a missing session or receipt collection, repair a replacement collection, or infer a new empty store.

### Unreleased coordination protocols 1 and 2

Coordination protocols 1 and 2 were experimental formats that were never released to production. Their authority handovers could write `rotating`, continue accepting cumulative usage from the old runtime, and later switch authority without recording the exact event position of that switch. Once such a history exists, neither the activation marker nor a protocol-1 receipt can reconstruct the missing boundary. Treating `rotating` as that boundary can overstate usage.

Protocol 3 therefore refuses every protocol-1 or protocol-2 activation phase, including `in-progress`, `complete`, and the experimental protocol-2 `upgrade-in-progress` phase. It refuses them whether or not `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` is set: the acknowledgement can prove that old writers stopped, but it cannot manufacture a historical accounting boundary. Protocol-3 `upgrade-in-progress` markers produced by the unshipped upgrade path are also refused because their shape does not prove which source history they came from. Startup performs no marker replacement, validator change, or document migration for any of these states.

The supported protocol-3 start states are a fresh or pre-coordination database entering initial activation, a protocol-3 `in-progress` marker resuming that initial activation, and a protocol-3 `complete` marker verifying an already activated database. Markerless activation requires the exact pre-coordination source shape: `sessions_v2` is still a collection, `sessions_v3` is absent, and no coordination-receipt collection or provenance exists. A coordinated namespace whose activation marker was lost is refused before replacement provenance can be inserted. `CHRONO_MONGODB_COORDINATION_CUTOVER=drained` remains the required acknowledgement for first activation of an existing pre-coordination database; it is not an old-protocol upgrade switch and cannot replace lost proof.

Current coordination receipts use protocol 2 only. Every completed authority-switch receipt must name a positive 64-bit `runtime_started_seq`, and startup verifies that event marker. A protocol-1 receipt, a receipt validator that still admits protocol 1, an older session protocol, or a downgraded validator makes even a protocol-3 `complete` store fail closed. CMA does not rewrite those records or silently repair the completed contract.

For an experimental database carrying any refused marker, preserve a transaction-consistent backup for diagnosis, then reset the whole experimental database or restore a whole transaction-consistent backup from before coordination was activated. Do not delete or synthesize an individual marker, rewrite receipts, or relabel validators to force startup. After restoring a genuine pre-coordination state, follow the initial activation procedure above and satisfy its offline drain before setting `CHRONO_MONGODB_COORDINATION_CUTOVER=drained`.

Rollback to coordination v1 or v2 is intentionally unsupported. An older startup rejects the protocol-3 marker, while an already-running old writer explicitly writes session protocol 1 or 2 and is rejected by the strict protocol-3 validator. Its coordination, delete, raw-append, and whole-document replacement transactions therefore abort rather than erase protocol-3 authority or its runtime-start marker. The offline drain remains mandatory because MongoDB validators cannot fence a standalone delete or recall a provider call. Never downgrade the validator or marker to make an old process start.

Rollback to a pre-coordination image is intentionally unsupported. Such an image expects `sessions_v2` to be a collection; after activation it is a view, so startup and direct old-style insert, replace, and delete operations must fail. Keep old workloads disabled after activation rather than treating that refusal as a reason to restore one.

If protected data or proof is lost, never create a new `session_coordination_receipts` collection and never synthesize or delete an activation marker to make startup pass. Restore `sessions_v3`, `session_events_v2`, `session_coordination_receipts`, all provenance records in `_cma_session_namespace_cutovers`, `session_writes`, and `session_tombstones` from the same consistent backup. Simultaneous loss of protected data and its durable proof is outside automatic recovery and requires an explicit incident recovery decision.

---

## 7. When it goes wrong

**The publish run failed before the images pushed.** Nothing was released — no tag exists, so nothing consumed the version. Fix the cause and re-run the workflow; it computes the same number again.

**It failed after the images pushed but before the tag.** The images exist at a version no tag claims. Re-running is still safe: it recomputes the same version and republishes those references from an equivalent build of the same commit. Nothing has been told to pull them, because the release that advertises them was never created — which is the window in which a reference may still be rewritten.

**A commit reached `main` outside a pull request.** The run fails deliberately: there is no label, so there is no declared bump, and guessing one would put a number on a release nobody sized. Either tag and publish by hand, or revert and merge it properly.

**The release pull request reads `BEHIND` and will not merge.** `develop` is missing the merge commit the *previous* release left on `main`. It carries no content — `git diff develop..main` is empty — but a merge commit is still a commit, and a protected branch requiring up-to-date branches will not take a pull request without it.

Since `v0.4.0` the publish workflow pushes a release-specific branch and opens that back-merge itself, as its last step, so this should not recur. If repository policy refuses Actions permission to open pull requests, the job fails and leaves the branch in place rather than reporting a false success. Open the branch against `develop` manually, then rerun the failed job; it recognizes the open pull request and completes without creating a duplicate. If the release gate later says `develop contains main` failed, merge that back-merge pull request. Merging `origin/main` into `develop` by hand does the same thing; do not force push, `develop` is protected.

A back-merge that **conflicts** is left open deliberately rather than resolved automatically: a conflict means `develop` moved during the release, and a resolution nobody chose is how a release silently reverts something.

**The push to GHCR is refused.** Two causes, in order of likelihood. The repository's default workflow permission is read-only — the explicit `permissions:` blocks should cover it, but confirm under Settings → Actions → General. Failing that, the organisation restricts package creation by Actions, which is not something this repository can change.

**Two merges landed together.** They do not race; the workflow serializes on a single concurrency group, and the second computes its version after the first has tagged.

**Three or more landed inside one build window.** A concurrency group holds one *pending* run, not a queue, so a third merge supersedes the second's pending run and that merge is marked cancelled having published nothing.

The code is not lost: the run that does publish is building a later commit on `main`, which contains it. What is lost is that merge's notes and its declared bump — so a `release:major` can be absorbed into a following `release:patch`, and the version that ships understates what is in it.

Re-run the cancelled run from the Actions tab before the next one tags and it publishes normally. After that, its number is no longer the next one, so fold its notes into the release that did go out instead. Rare by construction, since every merge into `main` is a release — but the only sign is a cancelled run rather than a failed one, which is easy to read as noise.
