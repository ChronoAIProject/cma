# Source license inventory

Generate a review draft from the locked source dependencies, without running
npm installs, package scripts or application builds. Use the repository's exact
Rust toolchain from `api/rust-toolchain.toml` and Node.js 22 or newer. No new
scanner dependency is required. The collector calls
`cargo metadata --locked --all-features --format-version 1` without a target
filter and reads **all five** tracked npm lockfiles, including dev, optional,
bundled and platform-specific package locations.

From the repository root, with Cargo on the current process's PATH:

```sh
mkdir -p .claude
PATH="$HOME/.cargo/bin:$PATH" CARGO_INCREMENTAL=0 \
  node .github/scripts/license-inventory.mjs --output .claude/license-draft
```

Cargo may fetch missing locked dependencies into its cache. For an isolated run,
set `CARGO_HOME` to a temporary directory under `.claude/` and remove that cache
after review. Use `--offline` when the cache is complete; missing metadata fails
the run. The tool does not install a toolchain and rejects `RUSTUP_TOOLCHAIN`
overrides or a Cargo version different from the exact pin. It neither invokes
`cargo deny` nor claims a license-policy gate pass.

The output directory must not exist; its parent must exist. Outputs are:

- `inventory.json`: normalized occurrences, input SHA-256 digests, tool versions,
  vendored file lists and unresolved declarations/notices.
- `THIRD_PARTY_NOTICES.md`: **draft** source-tree notices index.
- `COPYLEFT_REVIEW.md`: potential copyleft, weak-copyleft, attribution, font and
  restricted-redistribution obligations, with authoritative references.

Generated drafts are review artifacts, not committed assets. Keep them in the
ignored local directory or approved private custody. Do not put private reports,
registry credentials, environment identifiers or machine paths in this catalog.
The collector publishes digests of dependency origins instead of registry URLs.
It does not print raw Cargo diagnostics, which can contain local paths.

## Reproduce and interpret

Repeat against identical tracked inputs, dependency manifests, pinned Cargo and
Node versions, writing a new output directory, then compare all three outputs
with `diff`. There are no timestamps or absolute paths in the drafts. Input
hashes bind the lockfiles, workspace manifests, tool/policy/catalog and vendored
files; cached external Cargo manifest hashes bind the metadata declarations.
Policy and catalog parsing, hashing and report rendering share captured input
bytes. The policy snapshot is included in `inventory.json`; generation refuses
an input changed during collection. Every tracked notice filename is read and
hashed, so a missing tracked notice fails the run instead of disappearing.
Metadata collection reads the current tracked working tree, including edits,
so review the diff and use a clean commit for release evidence.

Npm rows count lockfile **locations**, not distinct packages and not an installed
platform graph. The root project entries are excluded; linked packages remain
explicit review rows. Cargo workspace members are separate from third-party
packages, even when their license metadata is absent. The inventory uses
`workspace-license-metadata-missing` for an absent workspace declaration;
the project license remains Apache-2.0. Cargo's all-features graph
is not a proof of every package in `Cargo.lock` or of a particular built artifact.
No target, dev-only, optional or build dependency is silently exempted.

License expressions are retained, including `AND`, `OR`, `WITH` and legacy Cargo
slash alternatives. The tool does not evaluate SPDX expressions or select a
license. `policy.json` is a conservative, replaceable triage catalog, **not an
allowlist**. Every unknown/custom expression, missing declaration or missing
notice requires review. Even a recognized permissive expression requires its
exact copyright, license and NOTICE texts to be collected before distribution.
Choosing a permissive alternative is a recorded review decision, never inferred
from a passing cargo-deny run.

`vendors.json` catalogs known copied code, derived assets, fonts and fixtures.
Prefix matches denote a review scope and can include locally authored support
files; they do not assign a license to every file in that directory. Missing
notice paths and unpinned origins stay visible. Additional license/notice
filenames are listed for review, but filename discovery cannot find all embedded
third-party code, native bundled libraries, media or generated derivatives.
Review those separately and extend the catalog with public provenance only.

## Release boundary

The root `LICENSE` grants Apache-2.0 for the approved first-party source scope;
this draft does not approve third-party redistribution. For each distributed
source archive, browser bundle,
binary or image, verify actual contents and gather the applicable license texts,
copyright/NOTICE material and covered-source or relinking instructions. Runtime
OS packages, downloaded tools, image layers and build-time fetches need separate
artifact inventories. The source draft is not an artifact SBOM or legal clearance.

## Checks

```sh
node --test --experimental-test-coverage \
  --test-coverage-include='.github/scripts/license-inventory*.mjs' \
  --test-coverage-include='.github/scripts/license-inventory/*.mjs' \
  --test-coverage-exclude='**/*.test.mjs' \
  --test-coverage-lines=80 --test-coverage-functions=80 \
  .github/scripts/license-inventory/*.test.mjs
```

Run `node --check` on the JavaScript modules and the existing public-tree
regression suite. A fresh real collection and byte-for-byte rerun complement the
synthetic tests. Check the generated drafts for unexpected paths before sharing.

Collector contracts: [Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
and [npm lockfiles](https://docs.npmjs.com/cli/v11/configuring-npm/package-lock-json/).
