# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
ARG SOURCE_DATE_EPOCH=0
ARG OMARCHY_CMA_BASE=runtime-tools
# Every Debian stage derives from debian-snapshot, which replaces the base
# image's live deb.debian.org sources with snapshot.debian.org at this one
# timestamp for trixie, trixie-updates and trixie-security. The same source
# therefore installs the same OS packages and rebuilds the OpenSSL CLI from
# the same Debian source. The snapshot's signed indexes are still verified
# against the archive keyring; only Valid-Until is waived, since an archived
# index outlives it by design.
#
# Security updates arrive only by bumping this value, deliberately:
#  1. Choose a time after the wanted archive state. Each archive serves its
#     latest snapshot at or before that time, so the time must already have
#     passed and snapshot.debian.org must list the debian and debian-security
#     snapshots it selects.
#  2. Rebuild linux/amd64. Exact version pins (chromium in install-os.sh,
#     BUBBLEWRAP_*) fail the build until they move with the snapshot. Diff
#     every Debian stage's dpkg status and the rebuilt binaries against the
#     previous build to list what moved.
#  3. A bump changes this file, so rebind its sha256 wherever it is pinned:
#     every build_inputs of .github/scripts/image-package-evidence.json and
#     image-omarchy-package-evidence.json, and dockerfile_sha256 in
#     deployables.json. Rebind each catalog row whose package or derived
#     output moved, from the signed indexes.
#  4. Land it only with an independent review of both catalogs, which sets
#     APPROVED_PACKAGE_EVIDENCE and APPROVED_OMARCHY_PACKAGE_EVIDENCE.
ARG DEBIAN_SNAPSHOT=20261124T215723Z
# The Omarchy stages install Arch packages exactly as the Arch Linux Archive
# indexed core, extra and multilib on this one date, so the same source
# installs the same packages and rebuilds the OpenSSL CLI, wlroots, Aquamarine
# and Hyprland with the same toolchain. They read every Arch and Omarchy
# package only from the OMARCHY_PACKAGES_IMAGE bundle below, mounted as
# file:// pacman repositories, so a cold build contacts no package server.
# pacman still verifies every package signature against the keyrings the build
# installs. The image keeps Omarchy's stable pacman.conf and mirrorlist for the
# sandbox's own installs.
#
# Arch updates arrive only by bumping this value, deliberately:
#  1. Choose a date whose following day the archive already lists, so the
#     chosen day's tree is complete; a published tree does not change.
#  2. Resolve every Omarchy install step offline against that date's
#     databases and regenerate .github/scripts/omarchy-packages.lock.json
#     with omarchy-package-bundle.py lock. The archive keeps the first upload
#     of each file name, so compare each resolved package's served size and
#     date with the index and list re-released packages with --reissued.
#     Re-pin Omarchy's own packages only to files its stable repository then
#     serves. Record the earliest expiry of every key that signed a resolved
#     package: a date stops building once one expires, because pacman checks
#     expiry against the build's clock. For 2026/10/22 the first is packager
#     subkey A85E811EB4CA2E08 at 2026-12-31T09:34:44Z.
#  3. Land the lock, dispatch omarchy-package-bundle.yml on develop with the
#     date, and pin the digest it prints in OMARCHY_PACKAGES_IMAGE.
#  4. Rebuild linux/amd64. Exact version checks (openssl in
#     build-arch-openssl-cli.sh, wlroots0.20 and aquamarine in
#     build-graphics.sh, hyprland in build-hyprland.sh, the pip wheel in
#     normalize-omarchy.py) fail the build until they move with the date. Diff
#     /usr/share/cma-omarchy/packages.txt and package-sha256.txt against the
#     previous build to list what moved.
#  5. Rebind this file's sha256 as for DEBIAN_SNAPSHOT, and each Omarchy
#     catalog row whose package or derived output moved, from the signed
#     packages.
#  6. Land it only with an independent review of both catalogs, which sets
#     APPROVED_PACKAGE_EVIDENCE and APPROVED_OMARCHY_PACKAGE_EVIDENCE.
ARG ARCH_SNAPSHOT=2026/10/22
# The bundle omarchy-package-bundle.yml built from that lock for ARCH_SNAPSHOT.
ARG OMARCHY_PACKAGES_IMAGE=ghcr.io/chronoaiproject/cma-omarchy-packages@sha256:7bb92b39ed0b56a97f9a071e854b90cb1327ab1cacde931edc3fb907ceb550e6
# CMA_MODEL_PROXY_PLACEHOLDER is supplied only in managed Agent launch plans,
# not in the Sandbox environment, so existing runtime identities stay stable.
# The sandbox provider schedules this image and starts the kernel-neutral Layer
# 4 supervisor. The image supplies the one supported agent provider, the tools
# needed to prepare git workspaces, and the NyxID CLI used through the local
# credential proxy.
# Cook the locked dependency graph in an exportable layer shared by both
# runtime targets. Fresh CI builders do not inherit BuildKit cache mounts.
FROM rust:1.96.1-alpine@sha256:a41f7740f8b45d45795624eec13a8b42263cc700f19f7e4e86e04d3dda08a479 AS compiler-base
RUN apk add --no-cache musl-dev
WORKDIR /build

FROM compiler-base AS chef
COPY api/rust-toolchain.toml ./
RUN CARGO_BUILD_JOBS=2 cargo install --locked --version 0.1.78 cargo-chef

FROM chef AS planner
COPY api/ ./
RUN cargo chef prepare --recipe-path /recipe.json

FROM chef AS dependencies
COPY --from=planner /recipe.json /recipe.json
RUN --mount=type=cache,id=cma-runtime-registry,target=/usr/local/cargo/registry,sharing=locked \
    CARGO_BUILD_JOBS=2 cargo chef cook --release --locked --bin layer4-runtime --recipe-path /recipe.json

FROM compiler-base AS build
COPY api/ ./
ARG TARGETARCH
# Rebuild every workspace crate from actual source, even when source timestamps
# predate the cooked placeholder artifacts. The dependency objects remain reusable.
# Statically linked against musl, so it runs on any base the runtime stage uses.
RUN --mount=type=cache,id=cma-runtime-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-runtime-target-${TARGETARCH},target=/build/target,sharing=locked \
    --mount=type=bind,from=dependencies,source=/build/target,target=/dependency-target \
    cp -a /dependency-target/. /build/target/ \
    && cargo clean --release --workspace --locked \
    && cargo build --jobs 2 --release --locked --bin layer4-runtime \
    && mkdir /out \
    && cp /build/target/release/layer4-runtime /out/layer4-runtime
# The base image carries only debian.sources; any other apt source is refused
# so no stage can resolve a package from a live mirror. Plain HTTP serves the
# image before ca-certificates exists; apt still verifies the signed indexes.
# A failed index download fails the build instead of silently resolving a
# smaller package set.
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS debian-snapshot
ARG DEBIAN_SNAPSHOT
RUN set -eu; \
    printf '%s\n' "${DEBIAN_SNAPSHOT}" | grep -Eqx '[0-9]{8}T[0-9]{6}Z'; \
    test ! -e /etc/apt/sources.list; \
    test "$(ls -A /etc/apt/sources.list.d)" = debian.sources; \
    archive=http://snapshot.debian.org/archive; \
    keyring=/usr/share/keyrings/debian-archive-keyring.pgp; \
    printf '%s\n' \
        'Types: deb' "URIs: ${archive}/debian/${DEBIAN_SNAPSHOT}" 'Suites: trixie trixie-updates' \
        'Components: main' "Signed-By: ${keyring}" 'Check-Valid-Until: no' '' \
        'Types: deb' "URIs: ${archive}/debian-security/${DEBIAN_SNAPSHOT}" 'Suites: trixie-security' \
        'Components: main' "Signed-By: ${keyring}" 'Check-Valid-Until: no' \
        > /etc/apt/sources.list.d/debian.sources; \
    printf '%s\n' 'Acquire::Retries "5";' 'APT::Update::Error-Mode "any";' \
        > /etc/apt/apt.conf.d/80cma-debian-snapshot
# Build the upstream GitHub release with patched Go and indirect modules.
# The toolchain includes the CVE-2026-111943 crypto/x509 fix.
# No CLI source or transport code is patched.
FROM debian-snapshot AS github-build
ARG TARGETARCH
ARG GH_VERSION=2.100.0
ARG GH_SOURCE_COMMIT=45437bc7eeeb3359bbfddd1742f79de7652fd3e2
ARG GH_SOURCE_SHA256=e16749bc0d99dc0633a3d5ebadf48ffff1c24beb1ce83e8f6a71bb64ce477e9a
ARG GH_GO_VERSION=1.26.11
ARG GH_GO_SHA256_AMD64=9849f4e04ee35303714e34fbc3ac8ab44ea3e448a95aa182f91e3b9b79c710ec
ARG GH_GO_SHA256_ARM64=6c2f29b458b5add0835f18b776f69c8881a6d88428f1484a6da55755e620757e
ARG GH_X_MOD_VERSION=0.41.0
ARG GH_X_NET_VERSION=0.62.0
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) want="${GH_GO_SHA256_AMD64}" ;; \
      arm64) want="${GH_GO_SHA256_ARM64}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/go.tar.gz "https://go.dev/dl/go${GH_GO_VERSION}.linux-${TARGETARCH}.tar.gz"; \
    echo "${want}  /tmp/go.tar.gz" | sha256sum -c -; \
    tar xzf /tmp/go.tar.gz -C /usr/local; \
    rm /tmp/go.tar.gz; \
    mkdir /source; \
    curl -fsSL -o /tmp/gh-source.tar.gz "https://github.com/cli/cli/archive/${GH_SOURCE_COMMIT}.tar.gz"; \
    echo "${GH_SOURCE_SHA256}  /tmp/gh-source.tar.gz" | sha256sum -c -; \
    tar xzf /tmp/gh-source.tar.gz -C /source --strip-components=1; \
    rm /tmp/gh-source.tar.gz
WORKDIR /source
ENV GOTOOLCHAIN=local CGO_ENABLED=0
RUN --mount=type=cache,id=cma-gh-modules,target=/root/go/pkg/mod,sharing=locked \
    --mount=type=cache,id=cma-gh-build-${TARGETARCH},target=/root/.cache/go-build,sharing=locked \
    /usr/local/go/bin/go mod edit -require="golang.org/x/mod@v${GH_X_MOD_VERSION}" -require="golang.org/x/net@v${GH_X_NET_VERSION}" \
    && /usr/local/go/bin/go mod download golang.org/x/mod \
    && /usr/local/go/bin/go mod download golang.org/x/net \
    && /usr/local/go/bin/go mod download all \
    && /usr/local/go/bin/go mod verify \
    && test "$(/usr/local/go/bin/go list -m -f '{{.Version}}' golang.org/x/mod)" = "v${GH_X_MOD_VERSION}" \
    && test "$(/usr/local/go/bin/go list -m -f '{{.Version}}' golang.org/x/net)" = "v${GH_X_NET_VERSION}" \
    && mkdir -p /out \
    && /usr/local/go/bin/go build -mod=readonly -trimpath \
        -ldflags="-s -w -X github.com/cli/cli/v2/internal/build.Version=${GH_VERSION} -X github.com/cli/cli/v2/internal/build.Date=2026-09-09" \
        -o /out/gh ./cmd/gh
# Preserve the installed Debian security release, its quilt patches, kernel
# baseline and hardening. Configure the already-present gVisor loopback
# idempotently and use upstream contained descriptor resolution only when
# openat2 is absent; guarded filesystem and network policies remain enforced.
FROM debian-snapshot AS bubblewrap-build
ARG DEBIAN_SNAPSHOT
ARG BUBBLEWRAP_VERSION=0.12.0
ARG BUBBLEWRAP_DEBIAN_VERSION=0.12.0-1~deb13u1
ARG BUBBLEWRAP_SOURCE_SHA256=9760d007363e3abba7c747489910f9f82d9fca53ba3bd3282e396fa3c97a3314
ARG BUBBLEWRAP_DEBIAN_SHA256=7b54f121aebf5d4b38360ea728274c26fa471582bc3e6658379b679ecd8b5a2f
ARG BUBBLEWRAP_DSC_SHA256=1abef77e6c35ce6c48c8969d31bffcc7a589bfd8e8fea9284fd8d77750f01d39
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl dpkg-dev libcap-dev libselinux1-dev \
        meson ninja-build patch pkgconf \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY .github/scripts/bubblewrap-loopback.patch .github/scripts/bubblewrap-loopback-test.c \
    .github/scripts/bubblewrap-openat2.patch .github/scripts/bubblewrap-openat2-test.c /build/
RUN set -eux; \
    base="https://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}/pool/updates/main/b/bubblewrap"; \
    curl -fsSL -o source.tar.xz "${base}/bubblewrap_${BUBBLEWRAP_VERSION}.orig.tar.xz"; \
    curl -fsSL -o debian.tar.xz "${base}/bubblewrap_${BUBBLEWRAP_DEBIAN_VERSION}.debian.tar.xz"; \
    curl -fsSL -o source.dsc "${base}/bubblewrap_${BUBBLEWRAP_DEBIAN_VERSION}.dsc"; \
    echo "${BUBBLEWRAP_SOURCE_SHA256}  source.tar.xz" | sha256sum -c -; \
    echo "${BUBBLEWRAP_DEBIAN_SHA256}  debian.tar.xz" | sha256sum -c -; \
    echo "${BUBBLEWRAP_DSC_SHA256}  source.dsc" | sha256sum -c -; \
    mv source.tar.xz "bubblewrap_${BUBBLEWRAP_VERSION}.orig.tar.xz"; \
    mv debian.tar.xz "bubblewrap_${BUBBLEWRAP_DEBIAN_VERSION}.debian.tar.xz"; \
    dpkg-source --no-check -x source.dsc /source
WORKDIR /source
RUN set -eux; \
    export DEB_BUILD_MAINT_OPTIONS=hardening=+pie,+bindnow; \
    dpkg-buildflags --export=sh > /build/compiler-flags; \
    . /build/compiler-flags; \
    export CFLAGS="$CFLAGS -DBWRAP_GVISOR_OPENAT_COMPAT=1"; \
    meson setup _build --prefix=/usr --buildtype=plain \
        -Dassume_kernel=5.10.0 -Dselinux=enabled -Dman=disabled \
        -Dbash_completion=disabled -Dzsh_completion=disabled -Dtests=false; \
    cc -O2 -Wall -Wextra -Werror -Wno-missing-field-initializers \
        -I. -I_build /build/bubblewrap-loopback-test.c -o /build/loopback-test; \
    if /build/loopback-test > /build/unpatched-test.log 2>&1; then \
        echo 'unpatched bubblewrap unexpectedly passed the regression test' >&2; exit 1; \
    fi; \
    grep -Fx 'FAIL preconfigured loopback reaches link setup: success=0, link requests=0' /build/unpatched-test.log; \
    patch --batch --fuzz=0 -p1 < /build/bubblewrap-loopback.patch; \
    cc -O2 -Wall -Wextra -Werror -Wno-missing-field-initializers \
        -I. -I_build /build/bubblewrap-loopback-test.c -o /build/loopback-test; \
    /build/loopback-test; \
    patch --batch --fuzz=0 -p1 < /build/bubblewrap-openat2.patch; \
    cc -O2 -Wall -Wextra -Werror -D_GNU_SOURCE -DBWRAP_GVISOR_OPENAT_COMPAT=1 \
        -Dsyscall=probe_syscall -I. -I_build -c safe_openat.c -o /build/safe_openat.o; \
    cc -O2 -Wall -Wextra -Werror -D_GNU_SOURCE -I. -I_build \
        /build/bubblewrap-openat2-test.c chroot_realpath.c /build/safe_openat.o \
        -o /build/openat2-test; \
    mkdir /newroot; \
    /build/openat2-test; \
    meson compile -C _build; \
    mkdir /out; \
    install -m 0755 _build/bwrap /out/bwrap; \
    printf '%s\n' "${BUBBLEWRAP_DEBIAN_VERSION}" > /out/debian-version; \
    test "$(/out/bwrap --version)" = "bubblewrap ${BUBBLEWRAP_VERSION}"

FROM debian-snapshot AS openssl-cli-build
RUN apt-get update && apt-get install -y --no-install-recommends build-essential dpkg-dev libzstd-dev patch perl zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
ARG OS_SECURITY_REFRESH=local
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/build-debian-openssl-cli.sh,target=/tmp/build-cli.sh,readonly --mount=type=bind,source=api/crates/layer4-runtime/image/verify-openssl-cli-hardening.sh,target=/tmp/verify-cli.sh,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/openssl-remove-benchmarks.patch,target=/tmp/openssl.patch,readonly \
    --mount=type=tmpfs,target=/build,size=4294967296 \
    test -n "${OS_SECURITY_REFRESH}" && sh /tmp/build-cli.sh /build /out /tmp/openssl.patch /tmp/verify-cli.sh

# The NyxID CLI is published as a *-unknown-linux-gnu binary.
FROM debian-snapshot AS runtime-system
ARG RUNTIME_DESKTOP=0
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/install-os.sh,target=/tmp/install-os.sh,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/normalize-machine-id.py,target=/tmp/normalize-machine-id.py,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/normalize-binutils-notices.py,target=/tmp/normalize-binutils-notices.py,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/remove-vendor-examples.py,target=/tmp/remove-vendor-examples.py,readonly \
    --mount=type=bind,from=openssl-cli-build,source=/out,target=/tmp/cma-openssl,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/install-debian-openssl-cli.sh,target=/tmp/install-openssl-cli.sh,readonly \
    sh /tmp/install-os.sh install \
    && groupadd --gid 10001 cma \
    && useradd --no-log-init --create-home --uid 10001 --gid 10001 --shell /bin/bash cma
FROM runtime-system AS runtime-tools
ARG NYXID_CLI_VERSION=v0.10.0
ARG CODEX_VERSION=0.165.0
ARG CODEX_UPSTREAM_NATIVE_SHA256_AMD64=c06a67fd8ac76508dc78bd99ce20c9f317b8d568beda59485f3f4134df9962d5
ARG CODEX_UPSTREAM_NATIVE_SHA256_ARM64=b32ece31af06440da980b733fb8fefdd1f193439595fda84096f1caaceec543f
ARG CODEX_LAUNCHER_SHA256=61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70
ARG CODEX_FORK_RELEASE=cma-codex-0.165.0-146878dacb49
ARG CODEX_FORK_PACKAGE_SHA256_AMD64=a42f99774031bcdfb5785554c0f5b458e463054e87df09d3d67851787f994ee8
ARG CODEX_FORK_PACKAGE_SHA256_ARM64=103cc138ebb72cadfab457f5eda807465c3e9842df68ca0948d245115c852389
ARG CODEX_NATIVE_SHA256_AMD64=1e9d2fb238b9c0d012c9b36e1ba1203ab5a8382ae937720320775c00d853d6dd
ARG CODEX_NATIVE_SHA256_ARM64=eb3dba09164f2aef23e7a32105f69ff5b172e2c717c4b6f75abc43faae183118
ARG CODEX_FORK_CODE_MODE_SHA256_AMD64=6d40220f43b937342f9cf7eaa50ba2965cc36b33ff8e36eda2760382e2691f25
ARG CODEX_FORK_CODE_MODE_SHA256_ARM64=d1108c33139eb38a2a73bfc6340128a0aa4d0e86dec9e8dea03a489d62139e99
ARG CODEX_FORK_BWRAP_SHA256_AMD64=fdedfbcc81efe3509735bfc73c51acc5a2da061ad7f816dd6ad3c332c973166d
ARG CODEX_FORK_BWRAP_SHA256_ARM64=2ec3517c8fd054be1f9c27a32b79c897fe88017a427d56852df08962dc49ecf3
ARG CODEX_FORK_RG_SHA256_AMD64=e62198eb19b136b88c330af83647b5a962cb99b6b1f066758568f12de1974849
ARG CODEX_FORK_RG_SHA256_ARM64=e36d0eb52e70696bdf1781392722e05a21bb91d3b7b762ef5ec20e5df2ec687b
ARG DESKTOP_CODEX_VERSION=0.165.0
ARG DESKTOP_CODEX_NATIVE_SHA256_AMD64=1e9d2fb238b9c0d012c9b36e1ba1203ab5a8382ae937720320775c00d853d6dd
ARG DESKTOP_CODEX_NATIVE_SHA256_ARM64=eb3dba09164f2aef23e7a32105f69ff5b172e2c717c4b6f75abc43faae183118
ARG DESKTOP_CODEX_LAUNCHER_SHA256=61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70
ARG CODEX_ACP_VERSION=1.4.0
ARG CODEX_ACP_UPSTREAM_SHA256=ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf
ARG CODEX_ACP_SHA256=11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4
# Node is downloaded from upstream and verified against digests recorded here.
# Both Codex packages are JavaScript launchers for pinned native/runtime code.
ARG NODE_VERSION=24.19.0
ARG NODE_SHA256_AMD64=14b342e71204f811bde6153be8e04b62aef63c236fef92b55f9c83154b409647
ARG NODE_SHA256_ARM64=01443c1e1a29e531ccad5a46fefa6df490d2189c49f7955904aecdbb0fe86fdc
ARG TARGETARCH
ARG RUNTIME_DESKTOP=0
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) arch="x64"; want="${NODE_SHA256_AMD64}" ;; \
      arm64) arch="arm64"; want="${NODE_SHA256_ARM64}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    tarball="node-v${NODE_VERSION}-linux-${arch}.tar.xz"; \
    curl -fsSL -o "/tmp/${tarball}" "https://nodejs.org/dist/v${NODE_VERSION}/${tarball}"; \
    echo "${want}  /tmp/${tarball}" | sha256sum -c -; \
    tar -xJf "/tmp/${tarball}" -C /usr/local --strip-components=1 \
        --exclude CHANGELOG.md --exclude LICENSE --exclude README.md; \
    rm -f "/tmp/${tarball}"; \
    node --version; \
    npm --version
# Keep the package manager useful for repository work while replacing the
# vulnerable packages bundled by this exact Node/npm line with pinned fixes.
ARG NPM_VERSION=12.0.2
ARG BRACE_EXPANSION_VERSION=5.0.12
ARG IP_ADDRESS_VERSION=10.3.1
ARG TAR_VERSION=7.5.21
ARG UNDICI_VERSION=6.29.1
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/npm,target=/tmp/cma-image-inputs/npm,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/install-npm.sh,target=/tmp/install-npm.sh,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/remove-vendor-examples.py,target=/tmp/remove-vendor-examples.py,readonly \
    sh /tmp/install-npm.sh \
    && python3 -B /tmp/remove-vendor-examples.py npm
# Install only the supported provider and its ACP bridge. Their package
# metadata, launcher, bridge source, and architecture-specific native binary
# are all checked against the reviewed pinned artifacts.
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/provider,target=/tmp/cma-image-inputs/provider,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/desktop-provider,target=/tmp/cma-image-inputs/desktop-provider,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/install-provider.sh,target=/tmp/install-provider.sh,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/install-codex-package.py,target=/tmp/install-codex-package.py,readonly \
    sh /tmp/install-provider.sh
# ACP's dependency can retain an older CLI. Always execute the verified global
# CLI selected above; this does not alter persisted Agent launch requests.
ENV CODEX_PATH=/usr/local/bin/codex
# Install the platform CLI from its pinned release and verify the published
# checksum before placing the binary on PATH.
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) target="x86_64-unknown-linux-gnu" ;; \
      arm64) target="aarch64-unknown-linux-gnu" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    mkdir /tmp/nyxid-cli; \
    base="https://github.com/ChronoAIProject/NyxID/releases/download/${NYXID_CLI_VERSION}"; \
    curl -fsSL -o /tmp/nyxid-cli/nyxid.tar.gz "${base}/nyxid-cli-${target}.tar.gz"; \
    curl -fsSL -o /tmp/nyxid-cli/nyxid.sha256 "${base}/nyxid-cli-${target}.tar.gz.sha256"; \
    ( cd /tmp/nyxid-cli && awk '{print $1"  nyxid.tar.gz"}' nyxid.sha256 | sha256sum -c - ); \
    tar xzf /tmp/nyxid-cli/nyxid.tar.gz -C /tmp/nyxid-cli; \
    install -m 0755 "$(find /tmp/nyxid-cli -type f -name nyxid -perm -u+x | head -1)" /usr/local/bin/nyxid; \
    rm -rf /tmp/nyxid-cli; \
    nyxid --version
# Use the genuine upstream CLI. The supervisor supplies an inert GH_TOKEN and
# its local Unix-socket transport to managed runtimes for live scope changes.
# Requests require the current api-github grant; the scoped NyxID credential
# remains in the supervisor, outside the home.
COPY --from=github-build /out/gh /usr/local/bin/gh
RUN gh --version
# Baseline skills are part of the runtime contract, so their source revisions
# and source archives are pinned just like executable dependencies. Additional
# skills are materialized per agent by the supervisor.
ARG NYXID_SKILL_COMMIT=0ac096837c7436b05b71b374207a42320776715d
ARG NYXID_SKILL_ARCHIVE_SHA256=6559072f4d4ab35b466cdf64ae0ae86ee3fb89a36f67925ed1ba4eb781ee23b8
ARG ORNN_SKILL_COMMIT=052024a6927306f2455843b850a948d77ac67f9d
ARG ORNN_SKILL_ARCHIVE_SHA256=a5788101f78d364e14b57bfc2fa1755702833b239b280ed6ce60c5abd3a442c3
# The pinned NyxID skill has two reviewed internal aliases. Fresh-volume
# initialization accepts only regular files/directories, so materialize
# these aliases in the immutable public seed while retaining the image home.
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/prepare-skill-docs.py,target=/tmp/prepare-skill-docs.py,readonly \
    set -eux; \
    install -d -o 10001 -g 10001 \
        /home/cma/.cma/agents \
        /home/cma/.cma/provider/codex \
        /home/cma/.cma/checkpoints \
        /home/cma/.codex/skills \
        /var/lib/cma-runtime /var/lib/cma-transfer; \
    mkdir /tmp/baseline-skills; \
    curl -fsSL -o /tmp/nyxid-source.tar.gz \
        "https://github.com/ChronoAIProject/NyxID/archive/${NYXID_SKILL_COMMIT}.tar.gz"; \
    echo "${NYXID_SKILL_ARCHIVE_SHA256}  /tmp/nyxid-source.tar.gz" | sha256sum -c -; \
    tar xzf /tmp/nyxid-source.tar.gz -C /tmp/baseline-skills --strip-components=2 \
        "NyxID-${NYXID_SKILL_COMMIT}/skills/nyxid"; \
    curl -fsSL -o /tmp/ornn-source.tar.gz \
        "https://github.com/ChronoAIProject/Ornn/archive/${ORNN_SKILL_COMMIT}.tar.gz"; \
    echo "${ORNN_SKILL_ARCHIVE_SHA256}  /tmp/ornn-source.tar.gz" | sha256sum -c -; \
    tar xzf /tmp/ornn-source.tar.gz -C /tmp/baseline-skills --strip-components=2 \
        "Ornn-${ORNN_SKILL_COMMIT}/skills/ornn-agent-manual-cli"; \
    python3 -B /tmp/prepare-skill-docs.py /tmp/baseline-skills; \
    cp -a /tmp/baseline-skills/nyxid /home/cma/.codex/skills/nyxid; \
    cp -a /tmp/baseline-skills/ornn-agent-manual-cli \
        /home/cma/.codex/skills/ornn-agent-manual-cli; \
    test -s /home/cma/.codex/skills/nyxid/SKILL.md; \
    test -s /home/cma/.codex/skills/ornn-agent-manual-cli/SKILL.md; \
    test "$(find /home/cma/.codex/skills -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort | paste -sd, -)" = "nyxid,ornn-agent-manual-cli"; \
    chown -R 10001:10001 /home/cma /var/lib/cma-runtime /var/lib/cma-transfer; \
    test ! -e /home/cma/.codex/auth.json; \
    test ! -e /home/cma/.nyxid/access_token; \
    test ! -e /home/cma/.cma/provider/codex/auth.json; \
    test "$(find /home/cma -type l -printf '%P\n' | sort | paste -sd, -)" = ".codex/skills/nyxid/POST_INSTALL.md,.codex/skills/nyxid/tools"; \
    test "$(readlink /home/cma/.codex/skills/nyxid/POST_INSTALL.md)" = "references/post-install.md"; \
    test "$(readlink /home/cma/.codex/skills/nyxid/tools)" = "scripts"; \
    cp -aL /home/cma /usr/share/cma-runtime-home; \
    channel_docs=.codex/skills/nyxid/references/channels.md; \
    cmp "/home/cma/$channel_docs" "/usr/share/cma-runtime-home/$channel_docs"; \
    test -z "$(find /usr/share/cma-runtime-home ! -type d ! -type f -print -quit)"; \
    chown -R root:root /usr/share/cma-runtime-home; \
    chmod -R a+rX,go-w /usr/share/cma-runtime-home; \
    rm -rf /tmp/baseline-skills /tmp/nyxid-source.tar.gz /tmp/ornn-source.tar.gz

# Native SandboxTemplates use this same image as a non-root init container:
# --initialize-home-tree /mnt/cma-home seeds home/cma and creates home/.codex;
# home mounts at /home, with separate persistent state/transfer directories.
# --initialize-home retains the v1 home -> /home/cma layout. Both versions
# reject the other's marker and preserve completed volumes across Pod recreation.

# Persist untouched provider conversations before publishing an Agent as Ready.
# These exact-source patches and restart proofs use synthetic credentials and
# local model/tool fixtures only. Native tools must survive rotation and reload.
RUN --mount=type=bind,source=api/crates/layer3-adapter/src/codex,target=/tmp/cma-codex-source,readonly \
    --mount=type=bind,source=api/crates/layer3-adapter/src/runtime_control_relay.py,target=/tmp/runtime_control_relay.py,readonly \
    node /tmp/cma-codex-source/persist_conversations.cjs \
    && node /tmp/cma-codex-source/subscription_auth.cjs \
    && node /tmp/cma-codex-source/managed_model_catalog.cjs \
    && node /tmp/cma-codex-source/runtime_observations.cjs \
    && node /tmp/cma-codex-source/required_access_tool.cjs \
    && node /tmp/cma-codex-source/session_commands.cjs \
    && node /tmp/cma-codex-source/agent_instructions.cjs \
    && node /tmp/cma-codex-source/turn_steering.cjs \
    && node /tmp/cma-codex-source/open_completion.cjs \
    && node /tmp/cma-codex-source/deferred_session_metadata.cjs \
    && node /tmp/cma-codex-source/session_open_overlap.cjs \
    && node /tmp/cma-codex-source/prompt_output_identity.cjs \
    && node /tmp/cma-codex-source/native_bound_dispatch.cjs \
    && echo "${CODEX_ACP_SHA256}  /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js" | sha256sum -c - \
    && test -x /usr/local/bin/codex-acp


# A single final payload changes with the supervisor and its browser/desktop assets.
FROM runtime-tools AS runtime-application-inputs
COPY --from=build /out/layer4-runtime /payload/usr/local/bin/layer4-runtime
COPY --chmod=755 api/crates/layer4-runtime/src/browser/command.cjs /payload/usr/local/bin/cma-browser
COPY api/crates/layer4-runtime/src/desktop-browser.py /payload/usr/local/bin/cma-desktop-browser
COPY --chmod=0755 api/crates/layer4-runtime/src/desktop-session.sh /payload/usr/local/bin/cma-desktop-session
COPY api/crates/layer4-runtime/src/desktop/ /payload/usr/share/cma-desktop/
COPY api/crates/layer4-runtime/src/desktop/chromium.desktop /payload/usr/share/applications/chromium.desktop

# Tests run against the whole current application payload without entering the tool layer.
FROM runtime-application-inputs AS runtime-qualification
RUN --mount=type=bind,source=api/crates/layer3-adapter/src/codex,target=/tmp/cma-codex-source,readonly \
    --mount=type=bind,source=api/crates/layer3-adapter/src/runtime_control_relay.py,target=/tmp/runtime_control_relay.py,readonly \
    cp -a /payload/. / \
    && case "${TARGETARCH}" in \
      amd64) platform=x64; target=x86_64-unknown-linux-musl; export CMA_CODEX_TEST_NATIVE_SHA256="${CODEX_NATIVE_SHA256_AMD64}";; \
      arm64) platform=arm64; target=aarch64-unknown-linux-musl; export CMA_CODEX_TEST_NATIVE_SHA256="${CODEX_NATIVE_SHA256_ARM64}";; \
      *) exit 1;; \
    esac \
    && export CMA_CODEX_TEST_NATIVE_BINARY="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-${platform}/vendor/${target}/bin/codex" \
    && export CMA_CODEX_TEST_LEGACY_BINARY="$(find /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/node_modules/@openai -path "*/vendor/${target}/bin/codex" -type f -print -quit)" \
    && test -x "$CMA_CODEX_TEST_LEGACY_BINARY" \
    && test "$("$CMA_CODEX_TEST_LEGACY_BINARY" --version)" = "codex-cli 0.147.0" \
    && node --test /tmp/cma-codex-source/native_stop_seal.test.cjs \
    && env CMA_RUNTIME_TEST_BINARY=/usr/local/bin/layer4-runtime CMA_CODEX_TEST_BINARY=/usr/local/bin/codex CMA_ACP_TEST_BRIDGE=/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js \
      node --test /tmp/cma-codex-source/repository_task.test.cjs /tmp/cma-codex-source/initial_task.test.cjs /tmp/cma-codex-source/suggested_reply.test.cjs /tmp/cma-codex-source/recorded_output.test.cjs /tmp/cma-codex-source/composer_completion.test.cjs /tmp/cma-codex-source/plan_progress.test.cjs \
    && node --test /tmp/cma-codex-source/session_open_overlap.test.cjs /tmp/cma-codex-source/native_process_identity.test.cjs /tmp/cma-codex-source/deferred_metadata_runtime.test.cjs /tmp/cma-codex-source/deferred_session_metadata.test.cjs \
    && node --test /tmp/cma-codex-source/prompt_output_buffer.test.cjs /tmp/cma-codex-source/prompt_output_identity.test.cjs /tmp/cma-codex-source/native_bound_dispatch.test.cjs \
    && node --test /tmp/cma-codex-source/persist_conversations.test.cjs /tmp/cma-codex-source/subscription_auth.test.cjs /tmp/cma-codex-source/native_tools.test.cjs /tmp/cma-codex-source/managed_model_catalog.test.cjs /tmp/cma-codex-source/live_skill_install.test.cjs /tmp/cma-codex-source/runtime_observations.test.cjs /tmp/cma-codex-source/required_access_tool.test.cjs /tmp/cma-codex-source/session_commands.test.cjs /tmp/cma-codex-source/agent_instructions.test.cjs /tmp/cma-codex-source/retained_context.test.cjs /tmp/cma-codex-source/endpoint_migration.test.cjs /tmp/cma-codex-source/turn_steering.test.cjs /tmp/cma-codex-source/open_completion.test.cjs /tmp/cma-codex-source/open_completion_native.test.cjs /tmp/cma-codex-source/recovery_rejoin_native.test.cjs \
    && python3 /tmp/cma-codex-source/recovery_test.py \
    && python3 /tmp/cma-codex-source/recovery_desktop_test.py \
    && node --test /tmp/cma-codex-source/recovery_native.test.cjs \
    && test ! -e /tmp/cma-checkpoints \
    && mkdir /out && printf 'qualified\n' > /out/qualification

FROM runtime-tools AS runtime-tool-payload
RUN mkdir -p /payload/usr/share /payload/usr /payload/etc \
    && cp -a /usr/local /payload/usr/ \
    && cp -a /usr/share/cma-runtime-home /payload/usr/share/ \
    && cp -a /home /payload/ \
    && if [ -d /etc/codex ]; then cp -a /etc/codex /payload/etc/; fi \
    && find /payload -exec touch -h -d '@0' '{}' +

FROM runtime-system AS runtime-refreshed
# Refresh OS files independently of the pinned tool payload; unchanged tool
# bytes remain reusable even when the OS branch is rebuilt. CI and release
# builds vary this value, so this branch and the OpenSSL CLI rebuild never come
# from a cached layer; they re-resolve the same DEBIAN_SNAPSHOT and reproduce
# the same bytes. Security updates arrive only by bumping DEBIAN_SNAPSHOT.
ARG OS_SECURITY_REFRESH=local
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/install-os.sh,target=/tmp/install-os.sh,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/normalize-machine-id.py,target=/tmp/normalize-machine-id.py,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/normalize-binutils-notices.py,target=/tmp/normalize-binutils-notices.py,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/remove-vendor-examples.py,target=/tmp/remove-vendor-examples.py,readonly \
    --mount=type=bind,from=openssl-cli-build,source=/out,target=/tmp/cma-openssl,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/install-debian-openssl-cli.sh,target=/tmp/install-openssl-cli.sh,readonly \
    sh /tmp/install-os.sh refresh

# Apply after the last security refresh so apt cannot overwrite the fix. A
# newer Debian package requires reviewing and repinning its source first.
# Preserve Debian's runtime dependencies and documentation. Codex resolves
# this system bwrap without changing the provider or permission profile.
RUN --mount=type=bind,from=bubblewrap-build,source=/out,target=/tmp/cma-bwrap,readonly \
    test "$(dpkg-query -W -f='${Version}' bubblewrap)" = "$(cat /tmp/cma-bwrap/debian-version)" \
    && install -m 0755 /tmp/cma-bwrap/bwrap /usr/bin/bwrap \
    && /usr/bin/bwrap --version

# Login shells keep the Sandbox Profile PATH even with a distinct Agent home.
# Match the pinned Debian stanza exactly so base-file drift fails the build.
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/login-path.py,target=/tmp/login-path.py,readonly \
    python3 /tmp/login-path.py \
    && rm -rf /var/log/apt/* /var/log/dpkg.log /var/log/alternatives.log /var/cache/ldconfig/aux-cache

FROM runtime-application-inputs AS runtime-application
RUN --mount=type=bind,from=runtime-qualification,source=/out/qualification,target=/tmp/qualification,readonly \
    --mount=type=bind,source=api/crates/layer4-runtime/image/assert-no-private-keyrings.py,target=/tmp/assert-no-private-keyrings.py,readonly \
    test "$(cat /tmp/qualification)" = qualified \
    && test -s /payload/usr/share/cma-desktop/wallpaper.png \
    && python3 -B /tmp/assert-no-private-keyrings.py /payload \
    && find /payload -exec touch -h -d '@0' '{}' +

FROM runtime-refreshed AS runtime
COPY --chmod=0755 api/crates/layer4-runtime/image/initialize-machine-id.py /usr/local/libexec/cma/initialize-sandbox
COPY --link --from=runtime-tool-payload /payload/ /
# Validate the base and the application payload separately; the final copy
# assembles only checked files and keeps application edits in the last layer.
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/assert-no-private-keyrings.py,target=/tmp/assert-no-private-keyrings.py,readonly \
    install -d -o 10001 -g 10001 /var/lib/cma-runtime /var/lib/cma-transfer \
    && ln -sfn /dev/shm/cma-machine-id/machine-id /etc/machine-id \
    && python3 -B /tmp/assert-no-private-keyrings.py
ENV CODEX_PATH=/usr/local/bin/codex


# Owner attachment uploads are admitted only when the supervisor proves the
# complete /home mount. Files and immutable upload receipts live beneath
# /home/attachments; retained runtimes advertise this capability explicitly.

# Optional CHRONO_TOOL_PROXY_URL selects one external HTTP tool endpoint.
# When absent no tool proxy binds. When present it listens only at 127.0.0.1:9093,
# accepts POST /, and uses the current in-memory Sandbox credential on each call.
# The control plane must retain the endpoint with the binding before enabling it.
WORKDIR /home/cma
USER 10001:10001
ENV HOME=/home/cma \
    CHRONO_RUNTIME_GENERATION=1 \
    CHRONO_RUNTIME_HOME_IDENTITY="" \
    PATH=/home/cma/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED=false \
    CHRONO_RUNTIME_AGENT_PROCESS_RESTART_ENABLED=true \
    CHRONO_RUNTIME_BIND=0.0.0.0:9090 \
    TERM=xterm-256color
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]

ARG CHRONO_BUILD_SHA=local-unpinned
ARG CHRONO_BUILD=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}" \
      dev.chrono.cma.runtime-contract="3" \
      dev.chrono.cma.runtime-desktop="${RUNTIME_DESKTOP}" \
      dev.chrono.cma.runtime-bootstrap="1" \
      dev.chrono.cma.home-layout="2"

COPY --link --from=runtime-application /payload/ /
FROM ${OMARCHY_CMA_BASE} AS omarchy-cma
FROM ${OMARCHY_PACKAGES_IMAGE} AS omarchy-packages
FROM archlinux:base@sha256:e82623936fa1a69700b48e98b058df6eaab18b737c27473a7234a40fdecb9413 AS omarchy-bootstrap
ARG ARCH_SNAPSHOT
COPY api/crates/layer4-runtime/image/build-omarchy-base.sh api/crates/layer4-runtime/image/normalize-omarchy.py api/crates/layer4-runtime/image/normalize-machine-id.py api/crates/layer4-runtime/image/assert-no-private-keyrings.py api/crates/layer4-runtime/image/build-arch-openssl-cli.sh api/crates/layer4-runtime/image/install-arch-openssl-cli.sh api/crates/layer4-runtime/image/openssl-remove-benchmarks.patch api/crates/layer4-runtime/image/verify-openssl-cli-hardening.sh /tmp/
RUN --mount=type=bind,from=omarchy-packages,source=/bundle,target=/run/cma-omarchy-packages,readonly bash /tmp/build-omarchy-base.sh /payload "${ARCH_SNAPSHOT}"
FROM scratch AS omarchy-base
COPY --link --from=omarchy-bootstrap /payload/ /
FROM omarchy-base AS omarchy-dependencies
ARG ARCH_SNAPSHOT
COPY api/crates/layer4-runtime/src/omarchy/install.sh /tmp/install-omarchy.sh
COPY api/crates/layer4-runtime/image/normalize-omarchy.py /tmp/normalize-omarchy.py
COPY api/crates/layer4-runtime/image/normalize-machine-id.py api/crates/layer4-runtime/image/install-arch-openssl-cli.sh /tmp/
COPY api/crates/layer4-runtime/image/assert-no-private-keyrings.py /tmp/assert-no-private-keyrings.py
COPY api/crates/layer4-runtime/src/omarchy/secure-tools.sh /tmp/secure-omarchy-tools.sh
COPY api/crates/layer4-runtime/src/omarchy/remove-google-account.patch /tmp/remove-google-account.patch
COPY api/crates/layer4-runtime/src/omarchy/build-graphics.sh api/crates/layer4-runtime/src/omarchy/wlroots-vkms.patch api/crates/layer4-runtime/src/omarchy/aquamarine-fullscreen.patch /tmp/
COPY api/crates/layer4-runtime/src/omarchy/build-hyprland.sh api/crates/layer4-runtime/src/omarchy/shm-allocator.hpp api/crates/layer4-runtime/src/omarchy/aquamarine-shm.patch api/crates/layer4-runtime/src/omarchy/hyprland-shm.patch /tmp/
RUN --mount=type=bind,from=omarchy-bootstrap,source=/arch-openssl-cli,target=/run/cma-openssl-cli,readonly --mount=type=bind,from=omarchy-packages,source=/bundle,target=/run/cma-omarchy-packages,readonly bash /tmp/install-omarchy.sh "${ARCH_SNAPSHOT}" && sh /tmp/install-arch-openssl-cli.sh /run/cma-openssl-cli / && python -B /tmp/normalize-machine-id.py && python -B /tmp/assert-no-private-keyrings.py && rm /tmp/install-arch-openssl-cli.sh /tmp/assert-no-private-keyrings.py /tmp/normalize-machine-id.py /tmp/normalize-omarchy.py /tmp/install-omarchy.sh /tmp/secure-omarchy-tools.sh /tmp/build-graphics.sh /tmp/build-hyprland.sh /tmp/shm-allocator.hpp /tmp/wlroots-vkms.patch /tmp/aquamarine-fullscreen.patch /tmp/aquamarine-shm.patch /tmp/hyprland-shm.patch
FROM omarchy-cma AS omarchy-tool-payload
USER 0
RUN mkdir -p /payload/usr/share /payload/usr/local /payload/etc \
    && cp -a /usr/local/bin /usr/local/lib /payload/usr/local/ \
    && rm -f /payload/usr/local/bin/layer4-runtime /payload/usr/local/bin/cma-browser \
        /payload/usr/local/bin/cma-desktop-browser /payload/usr/local/bin/cma-desktop-session \
    && cp -a /usr/share/cma-runtime-home /payload/usr/share/ \
    && cp -a /etc/codex /payload/etc/ \
    && find /payload -exec touch -h -d '@0' '{}' +
FROM runtime-application AS omarchy-application
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/initialize-keyring.py /payload/usr/local/libexec/cma/initialize-sandbox
COPY api/crates/layer4-runtime/src/omarchy/pacman-user.conf /payload/usr/share/cma-omarchy/pacman-user.conf
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/pacman-user.sh /payload/usr/local/bin/pacman
RUN ln -s pacman /payload/usr/local/bin/pacman-key && ln -s pacman /payload/usr/local/bin/pacman-conf
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/session.sh /payload/usr/local/bin/cma-desktop-session
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/inner-session.sh /payload/usr/local/bin/cma-omarchy-inner-session
COPY api/crates/layer4-runtime/src/omarchy/hyprland.lua /payload/usr/share/cma-omarchy/hyprland.lua
COPY api/crates/layer4-runtime/src/omarchy/graphics-device.py /payload/usr/share/cma-omarchy/graphics-device.py
COPY api/crates/layer4-runtime/src/omarchy/chromium.desktop /payload/usr/share/applications/chromium.desktop
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/assert-no-private-keyrings.py,target=/tmp/assert-no-private-keyrings.py,readonly \
    python3 -B /tmp/assert-no-private-keyrings.py /payload \
    && find /payload -exec touch -h -d '@0' '{}' +
FROM omarchy-dependencies AS omarchy
COPY --link --from=omarchy-tool-payload /payload/ /
RUN --mount=type=bind,source=api/crates/layer4-runtime/image/assert-no-private-keyrings.py,target=/tmp/assert-no-private-keyrings.py,readonly \
    groupadd --gid 10001 cma \
    && useradd --no-log-init --create-home --uid 10001 --gid 10001 --shell /bin/bash cma \
    && cp -a /usr/share/cma-runtime-home/. /home/cma/ \
    && channel_docs=.codex/skills/nyxid/references/channels.md \
    && python -B -c 'import sys; from pathlib import Path; sys.exit(Path(sys.argv[1]).read_bytes() != Path(sys.argv[2]).read_bytes())' "/home/cma/$channel_docs" "/usr/share/cma-runtime-home/$channel_docs" \
    && mkdir -p /var/lib/cma-runtime /var/lib/cma-transfer \
    && chown -R 10001:10001 /home/cma /var/lib/cma-runtime /var/lib/cma-transfer \
    && codex --version && node --version && nyxid --version && gh --version \
    && python -B /tmp/assert-no-private-keyrings.py
WORKDIR /home/cma
USER 10001:10001
ENV HOME=/home/cma CHRONO_RUNTIME_GENERATION=1 CHRONO_RUNTIME_HOME_IDENTITY="" \
    PATH=/home/cma/.local/bin:/usr/local/bin:/usr/share/omarchy/bin:/usr/bin \
    CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED=false \
    CHRONO_RUNTIME_AGENT_PROCESS_RESTART_ENABLED=true \
    CHRONO_RUNTIME_BIND=0.0.0.0:9090 TERM=xterm-256color \
    CODEX_PATH=/usr/local/bin/codex OMARCHY_PATH=/usr/share/omarchy
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]
ARG CHRONO_BUILD_SHA=local-unpinned
ARG CHRONO_BUILD=omarchy-prototype
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}" \
      dev.chrono.cma.runtime-contract="3" dev.chrono.cma.runtime-desktop="1" \
      dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2" \
      dev.chrono.cma.desktop-environment="omarchy-4.0.6" \
      dev.chrono.cma.omarchy-revision="237f1b4071a0bff2296c6d740ba2b9fd695ebfa0"

COPY --link --from=omarchy-application /payload/ /

FROM runtime AS default-runtime
ARG CHRONO_BUILD_SHA=local-unpinned
ARG CHRONO_BUILD=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"
