# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM golang:1.26.8-alpine3.23@sha256:33ce311e5eecedee48ec1b84419c1306e9fbd71009f0d5c3f2a6904b579c1ecc AS secret-scanner-build
ARG TARGETOS
ARG TARGETARCH
ENV GOTOOLCHAIN=local GOENV=off GOTELEMETRY=off \
    GOPROXY=https://proxy.golang.org GOSUMDB=sum.golang.org CGO_ENABLED=0
WORKDIR /src/gitleaks
# CMA security rebuild: unchanged 8.30.1 source/rules, patched Go dependencies.
# Literal pins cannot be replaced through build arguments. See docs/Secret_Scanner_Build.md.
RUN <<'PREPARE_SCANNER'
set -eu
case "${TARGETOS}/${TARGETARCH}" in
  linux/amd64|linux/arm64) ;;
  *) echo 'unsupported secret-scanner platform' >&2; exit 1 ;;
esac
test "$(go env GOVERSION)" = 'go1.26.8'
export GOOS="${TARGETOS}" GOARCH="${TARGETARCH}"
apk add --no-cache ca-certificates curl
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
  'https://codeload.github.com/gitleaks/gitleaks/tar.gz/83d9cd684c87d95d656c1458ef04895a7f1cbd8e' \
  --output /tmp/scanner.tar.gz
printf '%s\n' '6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115  /tmp/scanner.tar.gz' | sha256sum -c -
tar -xzf /tmp/scanner.tar.gz --strip-components=1 -C /src/gitleaks
printf '%s\n' 'e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml' | sha256sum -c -
go get golang.org/x/crypto@v0.55.0 golang.org/x/text@v0.41.0
go mod download all
go mod verify
mkdir -p /usr/local/share/gitleaks
printf '%s\n' \
  'e0edfb0862afb47842aee08811984801fef09c60865c77988c814a2b58450404  go.mod' \
  '8220b35a8d9ab7e2befe8880db36aded4fc68547ce5c2b42bb28c102d933e09c  go.sum' \
  'e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml' \
  > /usr/local/share/gitleaks/inputs.sha256
sha256sum -c /usr/local/share/gitleaks/inputs.sha256
printf '%s\n' \
  'CMA security rebuild of upstream Gitleaks 8.30.1' \
  'source=https://github.com/gitleaks/gitleaks' \
  'revision=83d9cd684c87d95d656c1458ef04895a7f1cbd8e' \
  'archive.sha256=6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115' \
  'builder=golang:1.26.8-alpine3.23@sha256:33ce311e5eecedee48ec1b84419c1306e9fbd71009f0d5c3f2a6904b579c1ecc' \
  > /usr/local/share/gitleaks/source.txt
PREPARE_SCANNER

RUN --network=none <<'BUILD_SCANNER'
set -eu
export GOOS="${TARGETOS}" GOARCH="${TARGETARCH}" GOPROXY=off
sha256sum -c /usr/local/share/gitleaks/inputs.sha256
go mod verify
go list -mod=readonly -m all > /src/gitleaks/modules.txt
# Upstream release archives use go-re2, selected by this build tag.
go build -mod=readonly -trimpath -buildvcs=false -tags gore2regex \
  -ldflags '-s -w -X=github.com/zricethezav/gitleaks/v8/version.Version=8.30.1' \
  -o /usr/local/bin/gitleaks .
sha256sum -c /usr/local/share/gitleaks/inputs.sha256
go version -m /usr/local/bin/gitleaks > /usr/local/share/gitleaks/buildinfo.txt
sha256sum /usr/local/bin/gitleaks > /usr/local/share/gitleaks/binary.sha256
BUILD_SCANNER

# Execute the version check on the target platform, not the native Go builder.
FROM alpine:3.22 AS secret-scanner
COPY --from=secret-scanner-build /usr/local/bin/gitleaks /usr/local/bin/gitleaks
COPY --from=secret-scanner-build /usr/local/share/gitleaks /usr/local/share/gitleaks
RUN --network=none test "$(/usr/local/bin/gitleaks version)" = '8.30.1'

# Native Codex owns hosted subscription OAuth and refresh. Keep this version
# identical to the runtime's tested authentication protocol.
FROM alpine:3.22 AS codex-auth
ARG TARGETARCH
ARG CODEX_VERSION=0.147.0
ARG CODEX_NATIVE_SHA256_AMD64=cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40
ARG CODEX_NATIVE_SHA256_ARM64=e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2
RUN apk add --no-cache curl ca-certificates
RUN set -eu; \
    case "${TARGETARCH}" in \
      amd64) platform=x64; triple=x86_64-unknown-linux-musl; want="${CODEX_NATIVE_SHA256_AMD64}" ;; \
      arm64) platform=arm64; triple=aarch64-unknown-linux-musl; want="${CODEX_NATIVE_SHA256_ARM64}" ;; \
      *) exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/codex.tgz "https://registry.npmjs.org/@openai/codex/-/codex-${CODEX_VERSION}-linux-${platform}.tgz"; \
    tar -xzf /tmp/codex.tgz -C /tmp "package/vendor/${triple}/bin/codex"; \
    cp "/tmp/package/vendor/${triple}/bin/codex" /usr/local/bin/codex-native; \
    echo "${want}  /usr/local/bin/codex-native" | sha256sum -c -; \
    test "$(/usr/local/bin/codex-native --version)" = "codex-cli ${CODEX_VERSION}"

FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
COPY docs/CMAEG_Integration.md docs/CMAEG_Architecture.md docs/CMAEG_Protocol.md /docs/
ARG TARGETARCH
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-api-target-${TARGETARCH},target=/build/target,sharing=locked \
    CHRONO_BUILD="${CHRONO_BUILD}" \
    CHRONO_BUILD_SHA="${CHRONO_BUILD_SHA}" \
    cargo build --release --locked --bin cmaeg \
    && mkdir /out \
    && cp /build/target/release/cmaeg /out/cmaeg

FROM alpine:3.22 AS runtime
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"
# Pull security fixes published after the base image was cut. ca-certificates is
# required, not optional: the control plane makes outbound HTTPS calls to the
# identity provider, and an empty trust store fails every one of them. CI and
# release builds change this value on every attempt so Docker cannot reuse an
# OS refresh layer after the package repository publishes a fix.
ARG OS_SECURITY_REFRESH=local
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apk upgrade --no-cache \
    && apk add --no-cache ca-certificates \
    && adduser -D -u 10001 chrono
USER 10001:10001
COPY --from=build /out/cmaeg /usr/local/bin/cmaeg
COPY --from=codex-auth /usr/local/bin/codex-native /usr/local/bin/codex-native
COPY --from=secret-scanner /usr/local/bin/gitleaks /usr/local/bin/gitleaks
COPY --from=secret-scanner /usr/local/share/gitleaks /usr/local/share/gitleaks
EXPOSE 8090
EXPOSE 9091
# CMAEG_METRICS_LISTEN defaults to 0.0.0.0:9091; /metrics is served separately
# from the public management, ingress and tools listener.
# Runtime contract: CMAEG_PUBLIC_URL, CMAEG_CMA_BASE_URL and CMAEG_NYXID_BASE_URL
# are required origins. Inject CMAEG_MASTER_KEYS and CMAEG_CMA_SERVICE_TOKEN
# from Kubernetes Secrets; startup refuses missing or invalid values before binding.
# CMAEG_CMA_CLIENT_ID and CMAEG_CMA_CLIENT_SECRET enable automatic outbound CMA
# token renewal; the service token then remains the separate inbound rekey bearer.
# CMAEG_TOOLS_NYXID_SLUG defaults to cmaeg. Channel cache settings default to
# CMAEG_CHANNEL_CACHE_TTL_MS=60000, CMAEG_CHANNEL_NEGATIVE_TTL_MS=10000,
# CMAEG_CHANNEL_CACHE_MAX=50000, CMAEG_RESTORE_RATE_PER_MINUTE=10 and
# CMAEG_RESTORE_TIMEOUT_MS=800. /readyz reports local readiness and CMA reachability.
# Relay callbacks use CMAEG_JWKS_TTL_MS=600000, CMAEG_INGRESS_MAX_BODY=1048576
# and CMAEG_INGRESS_INFLIGHT=256. POST /v1/callbacks/{channel_id} verifies callbacks.
# Turns use CMAEG_ACCEPT_BUDGET_MS=20000, CMAEG_TURN_TIMEOUT_MS=600000,
# CMAEG_BUSY_RETRY_MAX_MS=15000 and CMAEG_DRAIN_SECONDS=600. During shutdown,
# /readyz returns 503 draining while tracked turns finish within the drain deadline.
# Agent tools: /v1/events/{event_ref}/{replies,typing,context},
# /v1/messages/{message_ref}/update and stateless POST /mcp (2025-06-18).
# Providers retain event_context on the Responses POST and serve authenticated
# GET at the configured conversation path plus /events/{event_id}.
# Delivery uses CMAEG_NYXID_REPLY_TIMEOUT_MS=5000, CMAEG_ACK_TEXT="Working on it…",
# CMAEG_HEARTBEAT_MS=60000 and CMAEG_FAILURE_TEXT for short failure notices.
# Eight workers pace each relay route at three seconds and drain queued sends.
# Custom ingress: POST /v1/channels/{channel_id}/events (256 KiB), optional
# mode=stream SSE, and signed HTTPS result webhooks with DNS pinning per attempt.
# See docs/CMA_Operations.md.
ENTRYPOINT ["/usr/local/bin/cmaeg"]
# Health is asserted by the Kubernetes probes in infra/cmaeg/deployment.yaml.
