#!/usr/bin/env bash
set -euo pipefail
scale_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$scale_root"
scale_sha=$(cat evidence/binary-source-sha.txt)
[[ "$scale_sha" =~ ^[0-9a-f]{40}$ ]] || exit 2
gh auth status --active 2>&1 | grep -q 'account chronoai-shining ' || { echo 'BLOCKED: active gh account is not chronoai-shining' >&2; exit 1; }
test "$(git -C source rev-parse HEAD)" = "$scale_sha" || { echo 'Source changed since binary build' >&2; exit 1; }
test -z "$(git -C source status --porcelain)" || { echo 'Source checkout is dirty' >&2; exit 1; }
mkdir -p cache/package private/buildx
cp cache/binaries/cma cache/binaries/cmaeg cache/package/
python3 - <<'PY'
from pathlib import Path
recipe=Path('source/infra/api/Dockerfile').read_text()
boundary='FROM rust:1.96.1-alpine AS build'
if boundary not in recipe: raise SystemExit('Source build-stage contract changed; update the local package recipe')
prefix=recipe.split(boundary)[0]
final='''FROM alpine:3.22
ARG CHRONO_BUILD_SHA
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \\
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \\
      dev.chrono.scope="local-cma-scale-arm64-no-media-transformer"
RUN apk upgrade --no-cache && apk add --no-cache ca-certificates && adduser -D -u 10001 chrono
COPY cma cmaeg /usr/local/bin/
COPY --from=codex-auth /usr/local/bin/codex-native /usr/local/bin/codex-native
COPY --from=secret-scanner /usr/local/bin/gitleaks /usr/local/bin/gitleaks
COPY --from=secret-scanner /usr/local/share/gitleaks /usr/local/share/gitleaks
USER 10001:10001
EXPOSE 8080 8090 9091
ENTRYPOINT ["/usr/local/bin/cma"]
'''
Path('cache/package/Dockerfile').write_text(prefix+final)
PY
export BUILDX_CONFIG="$scale_root/private/buildx"
if ! docker buildx inspect cma-scale-package >/dev/null 2>&1; then
  docker buildx create --name cma-scale-package --driver docker-container \
    --driver-opt memory=3g,cpu-quota=200000 \
    --buildkitd-config manifests/buildkitd.toml
fi
docker buildx build --builder cma-scale-package --platform linux/arm64 --load \
  --build-arg "CHRONO_BUILD_SHA=$scale_sha" --tag "cma-scale-api:$scale_sha" cache/package
docker image inspect "cma-scale-api:$scale_sha" --format '{{json .Id}} {{json .Architecture}}' > evidence/image-identity.txt
bin/kind load docker-image --name cma-scale "cma-scale-api:$scale_sha"
python3 scripts/render-apps.py "cma-scale-api:$scale_sha" "$scale_sha"
scripts/k apply --dry-run=server -f manifests/apps.json >/dev/null
scripts/k apply -f manifests/apps.json
python3 scripts/migrate-eg.py
scripts/k rollout restart deployment/cma-agent-manager deployment/cma-event-gateway
scripts/k rollout status deployment/cma-agent-manager --timeout=300s
scripts/k rollout status deployment/cma-event-gateway --timeout=300s
