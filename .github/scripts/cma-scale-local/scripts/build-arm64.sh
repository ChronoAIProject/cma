#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
gh auth status --active 2>&1 | grep -q 'account chronoai-shining ' || { echo 'BLOCKED: active gh account is not chronoai-shining' >&2; exit 1; }
test "$(git -C source branch --show-current)" = 'tl/cma-am-replicas/integration' || { echo 'BLOCKED: source must be the authorized integration branch' >&2; exit 1; }
scale_sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)" || { echo 'Source checkout is dirty' >&2; exit 1; }
mkdir -p cache/cargo-registry cache/target cache/binaries evidence
docker run --rm --name cma-scale-build --platform linux/arm64 --cpus 2 --memory 5g --memory-swap 5g \
  --label dev.chrono.owner=local-env-manager-02 \
  -v "$scale_root/source:/src:ro" -v "$scale_root/cache/cargo-registry:/usr/local/cargo/registry" \
  -v "$scale_root/cache/target:/target" -v "$scale_root/cache/binaries:/out" -w /src/api \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=2 -e CHRONO_BUILD="local-scale-$scale_sha" \
  -e CHRONO_BUILD_SHA="$scale_sha" rust:1.96.1-alpine \
  sh -ec 'apk add --no-cache musl-dev; cargo build --release --locked --bin cma --bin cmaeg; cp /target/release/cma /target/release/cmaeg /out/'
printf '%s\n' "$scale_sha" > evidence/binary-source-sha.txt
shasum -a 256 cache/binaries/cma cache/binaries/cmaeg > evidence/binary-sha256.txt
