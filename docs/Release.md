# Release

A release is a merge into `main`. There is no separate step before it and none after it: the merge decides the version, publishes the images and writes the record.

This document is the procedure. The rules it implements are in [CLAUDE.md](../CLAUDE.md) §6.

---

## 1. What a release produces

| Artifact | Where | Immutable |
|---|---|---|
| `ghcr.io/chronoaiproject/chrono-bot-api:vX.Y.Z` | GHCR, private | yes |
| `ghcr.io/chronoaiproject/chrono-bot-web:vX.Y.Z` | GHCR, private | yes |
| `ghcr.io/chronoaiproject/chrono-bot-runtime:vX.Y.Z` | GHCR, private | yes |
| the same three at `:<short-sha>` | GHCR, private | yes |
| annotated tag `vX.Y.Z` | this repository | yes |
| GitHub release `vX.Y.Z` | this repository | notes editable |

The release page carries three things:

1. **The release notes**, under a `## Release Notes vX.Y.Z` heading the publisher writes, followed by the three subsections exactly as the pull request declared them.
2. **The source archives** — `Source code (zip)` and `Source code (tar.gz)`. GitHub generates these from the tag and attaches them to every release, so nothing uploads them and there is no second copy to go stale.
3. **The images** — every reference in a block you can copy, and a link to each package in GHCR.

No tag is ever moved and no tag is ever reused. There is deliberately no `latest`: a moving tag is something a manifest can be pointed at by accident, and §5 forbids a session sitting on an image whose contents can change underneath it.

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
gh api -X PATCH repos/ChronoAIProject/chrono-bot/branches/main/protection/required_status_checks \
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

The packages are private, so authenticate first. It must be a **classic** personal access token with the `read:packages` scope: GitHub Packages does not accept fine-grained tokens, and one fails with a 403 that reads like a missing permission rather than the wrong kind of credential.

```sh
echo "$GITHUB_TOKEN" | docker login ghcr.io -u <your-github-username> --password-stdin
docker pull ghcr.io/chronoaiproject/chrono-bot-api:v0.1.0
```

Inside Actions this does not arise — the workflow authenticates with the job's own `GITHUB_TOKEN`, which carries `packages: write` for this repository and needs no personal token at all.

The release body lists all three references for that version, which is the quickest way to get them exactly right.

To deploy into a cluster that cannot authenticate to GHCR, pull and re-push into the registry it does use. The published images carry no provenance attestation precisely so that this is a plain copy:

```sh
docker pull  ghcr.io/chronoaiproject/chrono-bot-api:v0.1.0
docker tag   ghcr.io/chronoaiproject/chrono-bot-api:v0.1.0 <target-registry>/chrono-bot-api:v0.1.0
docker push  <target-registry>/chrono-bot-api:v0.1.0
```

Only `web` is published for arm64 as well as amd64. `api` and `runtime` are amd64 only: emulated Rust release builds ran for hours in CI, and every node in the target cluster is amd64.

---

## 4. What a release deliberately does not do

**It does not deploy, and it does not touch `infra/`.** The manifests keep naming whatever image they already name. A release makes a version available; someone still has to decide to run it.

**It especially does not move `CHRONO_SANDBOX_IMAGE`.** Under §5 that value *is* the runtime version, and every session running a different one is rotated onto it — transcript snapshotted, sandbox replaced, agent resumed. Editing it is a rollout of every live session, which is why publishing a runtime image and adopting it are separate acts. Adopt one by editing `infra/api/configmap.yaml` in its own pull request, deliberately, when a rotation is what you want.

Changing an image reference in `infra/` is a deployment-surface change and belongs in the same pull request as anything else that changes with it (§4).

---

## 5. When it goes wrong

**The publish run failed before the images pushed.** Nothing was released — no tag exists, so nothing consumed the version. Fix the cause and re-run the workflow; it computes the same number again.

**It failed after the images pushed but before the tag.** The images exist at a version no tag claims. Re-running is still safe: it recomputes the same version and republishes those references from an equivalent build of the same commit. Nothing has been told to pull them, because the release that advertises them was never created — which is the window in which a reference may still be rewritten.

**A commit reached `main` outside a pull request.** The run fails deliberately: there is no label, so there is no declared bump, and guessing one would put a number on a release nobody sized. Either tag and publish by hand, or revert and merge it properly.

**The push to GHCR is refused.** Two causes, in order of likelihood. The repository's default workflow permission is read-only — the explicit `permissions:` blocks should cover it, but confirm under Settings → Actions → General. Failing that, the organisation restricts package creation by Actions, which is not something this repository can change.

**Two merges landed together.** They do not race; the workflow serializes on a single concurrency group, and the second computes its version after the first has tagged.

**Three or more landed inside one build window.** A concurrency group holds one *pending* run, not a queue, so a third merge supersedes the second's pending run and that merge is marked cancelled having published nothing.

The code is not lost: the run that does publish is building a later commit on `main`, which contains it. What is lost is that merge's notes and its declared bump — so a `release:major` can be absorbed into a following `release:patch`, and the version that ships understates what is in it.

Re-run the cancelled run from the Actions tab before the next one tags and it publishes normally. After that, its number is no longer the next one, so fold its notes into the release that did go out instead. Rare by construction, since every merge into `main` is a release — but the only sign is a cancelled run rather than a failed one, which is easy to read as noise.
