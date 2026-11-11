# API Cargo cache across CI runners

The API Dockerfile keeps Cargo's registry and release target directory in
BuildKit cache mounts. The existing `type=gha,mode=max` exporter saves ordinary
build layers, including the final `/out/cma` binary, but does not export those
mutable mounts. A new hosted runner therefore starts with empty Cargo mounts
when a source or build-identity change invalidates the release build layer.

CI now transfers only `cma-api-registry` and `cma-api-target-amd64` through
`.github/scripts/api-cargo-cache.py`. This is limited to the existing API image
job; other images and the release publisher keep their existing behavior.

## Transfer and acceptance order

1. Prepare a compatibility prefix, restore an archive, and validate its checksums,
   paths, member types and size. Missing or invalid archives start a cold build.
2. Inject the archives into the exact named mounts in the job's BuildKit builder.
   A unique build argument forces the bridge command to execute; a cached bridge
   layer cannot stand in for populating a new builder's mutable mounts.
   The bridge deliberately avoids `--no-cache`, which can prune cache mounts.
3. Run the existing API release build and native image load, with the same
   `--release --locked` Cargo command and build identity arguments.
4. Extract both mounts **before** the existing `docker buildx prune --all --force`.
   The normal image-preserving prune and vulnerability scan still execute.
5. Save the archive only after the API job has scanned and exported its tested
   native image. An exact cache hit does not need a second immutable upload.

All 15 check names and their dependencies remain unchanged. Cache hits never
skip a build, scan, runtime check or native image export, and cache contents
are not acceptance evidence. Only cache-service restore/save failures are
optional; all existing image checks retain their failure behavior.

## Identity, trust and bounds

The key contains API/Linux/amd64/musl identity, a weekly generation, and a hash
of the API Dockerfile, lockfile, workspace and crate manifests, Rust toolchain,
Cargo configuration and bridge implementation. The full source SHA distinguishes
immutable saves; restore falls back only within that compatibility prefix.
Ordinary Rust source edits reuse dependencies while Cargo checks changed source.
Dependency, compiler, manifest or bridge changes start a new cache generation.
Weekly rotation also bounds reuse across changes behind upstream base tags.

GitHub scopes caches to the key, archive version and ref, with default-branch
fallback. A PR merge-ref cache cannot seed `develop` or another PR. The workflow
and script restrict cache restore/injection to same-repository PR merge refs into
`develop`/`main`, or pushes to `develop` and `tl/release/**`. Release refs run the
full image qualification in their own concurrency group and may restore a
compatible default-branch cache. Only `develop` pushes can extract and save a
cache; the helper rejects extraction from release and PR refs before any cache
operation. Fork PRs build without this bridge.
No token or credential is passed into the cache-transfer containers.

Archives are limited to 1 GiB compressed, 4 GiB expanded and 200,000 members.
The bridge rejects absolute paths, parent traversal, symlinks and special files.
An empty mount is not saved as a warm cache. Each transfer has a 150-second
subprocess bound and three-minute workflow bound; cache actions also have
three-minute bounds. Extraction uses gzip level 1 to limit temporary disk use
before pruning. These limits can cause a cold build rather than cache reuse.
GitHub's normal cache quota and eviction still apply; the bridge does not delete
existing caches or grant additional workflow permissions.

The restore/save action is pinned to
`actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830` (v4.3.0).
Its `restore/action.yml`, `save/action.yml`, `src/restoreImpl.ts`,
`src/saveImpl.ts`, `src/utils/actionUtils.ts` and cache-scope documentation were
reviewed before selection. Explicit restore/save actions avoid end-of-job mount
extraction, which would run after this workflow has already pruned the builder.
The mount bridge is repository source; its Alpine tool image is digest-pinned.

## Validation and expected benefit

The existing release-contract job runs fast Python regressions for authority,
cache invalidation, archive safety, size bounds, cache misses and timeouts.
Workflow mutation tests reject widened restore prefixes, fork access, mutable
action pins, extraction after prune, saving after failed image checks, and
skipping build or scan on a cache hit.

Before accepting the change, run the standalone fresh-builder proof on a
machine with a working Docker daemon:

```sh
python3 .github/scripts/api-cargo-cache-tests.py
ruby .github/scripts/ci-contract.rb release
ruby .github/scripts/ci-contract-tests.rb release
python3 .github/scripts/api-cargo-cache-integration.py
```

The integration script creates two disposable builders without changing the
selected builder. It builds a tiny Rust workspace, extracts both mounts,
removes the first builder, and restores into the second. It then changes the
application source and requires both a Cargo dependency cache hit and changed
executable output (`42` to `43`). It removes only the builders it created. This
proof is manual so recurring CI does not pay for two additional builders.

The first eligible run, each invalidated generation, and any evicted or oversized
cache still require cold compilation. Warm runs pay archive transfer/compression
costs and still compile changed crates. No measured API-job speedup is claimed
until a cold run and subsequent compatible run complete on hosted CI; the
Backend plus latency-gate path may remain the overall workflow bottleneck.
