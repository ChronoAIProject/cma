# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM golang:1.26.11-alpine3.23@sha256:23f198931d4bd83b4f5a613bd75656257ae8bf832dcbc2db80e1050b5219ca2a AS secret-scanner-build
ARG TARGETOS
ARG TARGETARCH
ENV GOTOOLCHAIN=local GOENV=off GOTELEMETRY=off \
    GOPROXY=https://proxy.golang.org GOSUMDB=sum.golang.org CGO_ENABLED=0
WORKDIR /src/gitleaks
# CMA security rebuild: unchanged 8.30.1 source/rules, patched Go dependencies.
# Literal pins cannot be replaced through build arguments. See private maintainer guidance.
RUN <<'PREPARE_SCANNER'
set -eu
case "${TARGETOS}/${TARGETARCH}" in
  linux/amd64|linux/arm64) ;;
  *) echo 'unsupported secret-scanner platform' >&2; exit 1 ;;
esac
test "$(go env GOVERSION)" = 'go1.26.11'
export GOOS="${TARGETOS}" GOARCH="${TARGETARCH}"
apk add --no-cache ca-certificates curl
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
  'https://codeload.github.com/gitleaks/gitleaks/tar.gz/83d9cd684c87d95d656c1458ef04895a7f1cbd8e' \
  --output /tmp/scanner.tar.gz
printf '%s\n' '6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115  /tmp/scanner.tar.gz' | sha256sum -c -
tar -xzf /tmp/scanner.tar.gz --strip-components=1 -C /src/gitleaks
printf '%s\n' 'e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml' | sha256sum -c -
go get golang.org/x/crypto@v0.55.0 golang.org/x/text@v0.41.0 github.com/klauspost/compress@v1.18.7
go mod download all
go mod verify
mkdir -p /usr/local/share/gitleaks
printf '%s\n' \
  '0d21d8af0e74395d5b88a1fd5ca4f421818c45b3f7a2b38100566eeca61ad675  go.mod' \
  'c1f21f073cc203c7678f230b9cb14131a25729f3e2fdbd73475d0de44be5711e  go.sum' \
  'e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml' \
  > /usr/local/share/gitleaks/inputs.sha256
sha256sum -c /usr/local/share/gitleaks/inputs.sha256
printf '%s\n' \
  'CMA security rebuild of upstream Gitleaks 8.30.1' \
  'source=https://github.com/gitleaks/gitleaks' \
  'revision=83d9cd684c87d95d656c1458ef04895a7f1cbd8e' \
  'archive.sha256=6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115' \
  'builder=golang:1.26.11-alpine3.23@sha256:23f198931d4bd83b4f5a613bd75656257ae8bf832dcbc2db80e1050b5219ca2a' \
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

# Compile dependency objects in an exportable layer. Cache mounts alone are
# not part of BuildKit's layer export on a fresh runner.
FROM rust:1.96.1-alpine AS chef
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/rust-toolchain.toml ./
RUN cargo install --locked --version 0.1.78 cargo-chef

FROM chef AS planner
COPY api/ ./
RUN cargo chef prepare --recipe-path /recipe.json

FROM chef AS dependencies
COPY --from=planner /recipe.json /recipe.json
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    cargo chef cook --release --locked --bin cma --recipe-path /recipe.json \
    && cargo clean --release --workspace --locked

FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache binutils musl-dev python3
WORKDIR /build
COPY api/ ./
ARG TARGETARCH
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
# Validate restored workspace objects against the copied sources before Cargo
# sees their fingerprints. Keep the registry dependency stage independent.
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-api-target-${TARGETARCH},target=/build/target,sharing=locked \
    --mount=type=bind,from=dependencies,source=/build/target,target=/dependency-target \
    export CARGO_INCREMENTAL=1 \
    && export RUSTC_WRAPPER=/build/build_support/release_rustc.py \
    && python3 build_support/workspace_cache.py prepare cma --dependency-target /dependency-target \
    && cargo build --release --locked --bin cma \
      --verbose --config profile.release.package.cma.codegen-units=256 \
    && mkdir /out \
    && cp /build/target/release/cma /out/cma \
    && python3 build_support/workspace_cache.py finish cma
# Strip only the shipped ELF, leaving Cargo cache artifacts intact.
RUN --network=none strip --strip-all /out/cma

# Exercise the actual API's mimalloc before the runtime can copy its ELF.
# Static musl/linker metadata does not establish bounded allocator retention.
RUN --network=none /out/cma --allocator-self-test

FROM alpine:3.22 AS runtime
ARG CHRONO_BUILD=development
ARG CHRONO_BUILD_SHA=development
LABEL org.opencontainers.image.title="CMA Agent Manager" \
      org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
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
COPY --from=build /out/cma /usr/local/bin/cma
COPY --from=codex-auth /usr/local/bin/codex-native /usr/local/bin/codex-native
COPY --from=secret-scanner /usr/local/bin/gitleaks /usr/local/bin/gitleaks
COPY --from=secret-scanner /usr/local/share/gitleaks /usr/local/share/gitleaks
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cma"]
# Operators configure health probes outside this repository.
