# syntax=docker/dockerfile:1
ARG OMARCHY_CMA_BASE=runtime
# CMA_MODEL_PROXY_PLACEHOLDER is supplied only in managed Agent launch plans,
# not in the Sandbox environment, so existing runtime identities stay stable.
# The sandbox provider schedules this image and starts the kernel-neutral Layer
# 4 supervisor. The image supplies the one supported agent provider, the tools
# needed to prepare git workspaces, and the NyxID CLI used through the local
# credential proxy.
FROM rust:1.96.1-alpine@sha256:a41f7740f8b45d45795624eec13a8b42263cc700f19f7e4e86e04d3dda08a479 AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
ARG TARGETARCH
# Statically linked against musl, so it runs on any base the runtime stage uses.
RUN --mount=type=cache,id=cma-runtime-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cma-runtime-target-${TARGETARCH},target=/build/target,sharing=locked \
    cargo build --jobs 2 --release --locked --bin layer4-runtime \
    && mkdir /out \
    && cp /build/target/release/layer4-runtime /out/layer4-runtime

# Build the upstream GitHub release with patched Go and indirect modules.
# The toolchain includes the CVE-2026-111943 crypto/x509 fix.
# No CLI source or transport code is patched.
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS github-build
ARG TARGETARCH
ARG GH_VERSION=2.100.0
ARG GH_SOURCE_COMMIT=45437bc7eeeb3359bbfddd1742f79de7652fd3e2
ARG GH_SOURCE_SHA256=e16749bc0d99dc0633a3d5ebadf48ffff1c24beb1ce83e8f6a71bb64ce477e9a
ARG GH_GO_VERSION=1.26.10
ARG GH_GO_SHA256_AMD64=ccfe7ae822d2c2f41f4fe7c956199967a4dac22d0ee0d2b2bfdb440aab3b20c5
ARG GH_GO_SHA256_ARM64=3f4227ce7b206a513e794f1393adb473a07b46e94d18bd15d1c684bbb7319797
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
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS bubblewrap-build
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
    base=https://security.debian.org/debian-security/pool/updates/main/b/bubblewrap; \
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

# The NyxID CLI is published as a *-unknown-linux-gnu binary.
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS runtime

ARG NYXID_CLI_VERSION=v0.10.0
ARG CODEX_VERSION=0.147.0
ARG CODEX_NATIVE_SHA256_AMD64=cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40
ARG CODEX_NATIVE_SHA256_ARM64=e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2
ARG CODEX_LAUNCHER_SHA256=134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477
ARG DESKTOP_CODEX_VERSION=0.165.0
ARG DESKTOP_CODEX_NATIVE_SHA256_AMD64=c06a67fd8ac76508dc78bd99ce20c9f317b8d568beda59485f3f4134df9962d5
ARG DESKTOP_CODEX_NATIVE_SHA256_ARM64=b32ece31af06440da980b733fb8fefdd1f193439595fda84096f1caaceec543f
ARG DESKTOP_CODEX_LAUNCHER_SHA256=61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70
ARG CODEX_ACP_VERSION=1.4.0
ARG CODEX_ACP_UPSTREAM_SHA256=ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf
ARG CODEX_ACP_SHA256=ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66

# Node is downloaded from upstream and verified against digests recorded here.
# Both Codex packages are JavaScript launchers for pinned native/runtime code.
ARG NODE_VERSION=24.19.0
ARG NODE_SHA256_AMD64=14b342e71204f811bde6153be8e04b62aef63c236fef92b55f9c83154b409647
ARG NODE_SHA256_ARM64=01443c1e1a29e531ccad5a46fefa6df490d2189c49f7955904aecdbb0fe86fdc
ARG TARGETARCH
ARG RUNTIME_DESKTOP=0

# The root filesystem is read-only in production and the runtime user is not
# privileged. User-space installers therefore write under /home/cma. Python
# and the archive tools are present for those installers; git prepares CMA
# workspaces; bubblewrap is the distro path Codex expects on restricted hosts.
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
        bash \
        bubblewrap \
        ca-certificates \
        chromium=156.0.8078.79-1~deb13u1 \
        chromium-common=156.0.8078.79-1~deb13u1 \
        chromium-sandbox=156.0.8078.79-1~deb13u1 \
        curl \
        build-essential \
        git \
        gzip \
        libdbus-1-3 \
        libgmp-dev \
        libgssapi-krb5-2 \
        libicu76 \
        libssl3t64 \
        libunwind8 \
        ncurses-term \
        python3 \
        ripgrep \
        tar \
        unzip \
        xz-utils \
        zlib1g \
        zstd \
    && if [ "${RUNTIME_DESKTOP}" = "1" ]; then apt-get install -y --no-install-recommends chromium dbus-x11 fonts-liberation novnc scrot wmctrl x11vnc x11-xserver-utils x11-utils xdotool xvfb websockify xfce4-session xfce4-panel xfdesktop4 xfwm4 xfce4-settings xfce4-appfinder gvfs thunar xfce4-terminal adwaita-icon-theme librsvg2-common; else test "${RUNTIME_DESKTOP}" = "0"; fi \
    && git --version \
    && rm -rf /var/lib/apt/lists/*

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
RUN set -eux; \
    npm install -g "npm@${NPM_VERSION}"; \
    bundled=/usr/local/lib/node_modules/npm/node_modules; \
    npm install --no-save --prefix /tmp/npmfix \
        "brace-expansion@${BRACE_EXPANSION_VERSION}" \
        "ip-address@${IP_ADDRESS_VERSION}" \
        "tar@${TAR_VERSION}" \
        "undici@${UNDICI_VERSION}"; \
    for package in brace-expansion ip-address tar undici; do \
      rm -rf "$bundled/$package"; \
      cp -a "/tmp/npmfix/node_modules/$package" "$bundled/$package"; \
    done; \
    test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/tar/package.json').version")" = "${TAR_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/brace-expansion/package.json').version")" = "${BRACE_EXPANSION_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/undici/package.json').version")" = "${UNDICI_VERSION}"; \
    rm -rf /tmp/npmfix; \
    npm cache clean --force; \
    npm --version

# Install only the supported provider and its ACP bridge. Their package
# metadata, launcher, bridge source, and architecture-specific native binary
# are all checked against the reviewed pinned artifacts.
RUN set -eux; \
    if [ "${RUNTIME_DESKTOP}" = "1" ]; then \
      CODEX_VERSION="${DESKTOP_CODEX_VERSION}"; \
      CODEX_NATIVE_SHA256_AMD64="${DESKTOP_CODEX_NATIVE_SHA256_AMD64}"; \
      CODEX_NATIVE_SHA256_ARM64="${DESKTOP_CODEX_NATIVE_SHA256_ARM64}"; \
      CODEX_LAUNCHER_SHA256="${DESKTOP_CODEX_LAUNCHER_SHA256}"; \
      mkdir -p /etc/codex; \
      printf '[features]\ncomputer_use = true\n' > /etc/codex/config.toml; \
    fi; \
    npm install -g "@openai/codex@${CODEX_VERSION}" \
        "@agentclientprotocol/codex-acp@${CODEX_ACP_VERSION}"; \
    npm cache clean --force; \
    test "$(node -p "require('/usr/local/lib/node_modules/@openai/codex/package.json').version")" = "${CODEX_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "${CODEX_ACP_VERSION}"; \
    test "$(codex --version)" = "codex-cli ${CODEX_VERSION}"; \
    echo "${CODEX_LAUNCHER_SHA256}  /usr/local/lib/node_modules/@openai/codex/bin/codex.js" | sha256sum -c -; \
    echo "${CODEX_ACP_UPSTREAM_SHA256}  /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js" | sha256sum -c -; \
    case "${TARGETARCH}" in \
      amd64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_AMD64}" ;; \
      arm64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_ARM64}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    echo "${want}  ${native}" | sha256sum -c -

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
RUN set -eux; \
    groupadd --gid 10001 cma; \
    useradd --create-home --uid 10001 --gid 10001 --shell /bin/bash cma; \
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
    test -z "$(find /usr/share/cma-runtime-home ! -type d ! -type f -print -quit)"; \
    chown -R root:root /usr/share/cma-runtime-home; \
    chmod -R a+rX,go-w /usr/share/cma-runtime-home; \
    rm -rf /tmp/baseline-skills /tmp/nyxid-source.tar.gz /tmp/ornn-source.tar.gz

COPY --from=build /out/layer4-runtime /usr/local/bin/layer4-runtime
COPY --chmod=755 api/crates/layer4-runtime/src/browser/command.cjs /usr/local/bin/cma-browser

# A Machine Included Sandbox starts this rootless desktop session from the
# supervisor.  noVNC is bound to loopback; CMA must relay it through the
# authenticated control plane before a browser can reach the screen.
COPY api/crates/layer4-runtime/src/desktop-browser.py /usr/local/bin/cma-desktop-browser
COPY --chmod=0755 api/crates/layer4-runtime/src/desktop-session.sh /usr/local/bin/cma-desktop-session
COPY api/crates/layer4-runtime/src/desktop/ /usr/share/cma-desktop/
# Retained XFCE homes may still reference the original wallpaper asset.
RUN test -s /usr/share/cma-desktop/wallpaper.png
COPY api/crates/layer4-runtime/src/desktop/chromium.desktop /usr/share/applications/chromium.desktop

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
    && echo "${CODEX_ACP_SHA256}  /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js" | sha256sum -c - \
    && test -x /usr/local/bin/codex-acp \
    && node --test /tmp/cma-codex-source/persist_conversations.test.cjs /tmp/cma-codex-source/subscription_auth.test.cjs /tmp/cma-codex-source/native_tools.test.cjs /tmp/cma-codex-source/managed_model_catalog.test.cjs /tmp/cma-codex-source/live_skill_install.test.cjs /tmp/cma-codex-source/runtime_observations.test.cjs /tmp/cma-codex-source/required_access_tool.test.cjs /tmp/cma-codex-source/session_commands.test.cjs /tmp/cma-codex-source/agent_instructions.test.cjs /tmp/cma-codex-source/retained_context.test.cjs /tmp/cma-codex-source/endpoint_migration.test.cjs /tmp/cma-codex-source/turn_steering.test.cjs /tmp/cma-codex-source/open_completion.test.cjs /tmp/cma-codex-source/open_completion_native.test.cjs \
    && python3 /tmp/cma-codex-source/recovery_test.py \
    && python3 /tmp/cma-codex-source/recovery_desktop_test.py \
    && node --test /tmp/cma-codex-source/recovery_native.test.cjs \
    && test ! -e /tmp/cma-checkpoints

# Refresh the final filesystem after expensive dependency layers are cached.
# CI and release builds vary this value so cached OS packages cannot hide a
# newly published security update from the image scan.
ARG OS_SECURITY_REFRESH=local
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apt-get update \
    && apt-get upgrade -y \
    && rm -rf /var/lib/apt/lists/*

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
RUN node <<'NODE'
const assert = require('node:assert/strict');
const fs = require('node:fs');
const profile = fs.readFileSync('/etc/profile', 'utf8');
const defaults = [
  'if [ "$(id -u)" -eq 0 ]; then',
  '  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"',
  'else',
  '  PATH="/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games"',
  'fi',
  'export PATH',
].join('\n');
assert.equal(profile.split(defaults).length, 2, 'unexpected Debian login PATH defaults');
const fallback = [
  'if [ -z "${PATH:-}" ]; then',
  ...defaults.split('\n').slice(0, -1).map((line) => `  ${line.replace('$(id -u)', '$(/usr/bin/id -u)')}`),
  'fi',
  'export PATH',
].join('\n');
fs.writeFileSync('/etc/profile', profile.replace(defaults, fallback));
NODE

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

# Explicit experimental target. The default target remains the Debian runtime.
# CI supplies a verified native CMA artifact as OMARCHY_CMA_BASE.
FROM ${OMARCHY_CMA_BASE} AS omarchy-cma
FROM archlinux:base@sha256:e82623936fa1a69700b48e98b058df6eaab18b737c27473a7234a40fdecb9413 AS omarchy
COPY api/crates/layer4-runtime/src/omarchy/install.sh /tmp/install-omarchy.sh
COPY api/crates/layer4-runtime/src/omarchy/secure-tools.sh /tmp/secure-omarchy-tools.sh
COPY api/crates/layer4-runtime/src/omarchy/build-graphics.sh api/crates/layer4-runtime/src/omarchy/wlroots-vkms.patch api/crates/layer4-runtime/src/omarchy/aquamarine-fullscreen.patch /tmp/
COPY api/crates/layer4-runtime/src/omarchy/build-hyprland.sh api/crates/layer4-runtime/src/omarchy/shm-allocator.hpp api/crates/layer4-runtime/src/omarchy/aquamarine-shm.patch api/crates/layer4-runtime/src/omarchy/hyprland-shm.patch /tmp/
RUN bash /tmp/install-omarchy.sh && rm /tmp/install-omarchy.sh /tmp/secure-omarchy-tools.sh /tmp/build-graphics.sh /tmp/build-hyprland.sh /tmp/shm-allocator.hpp /tmp/wlroots-vkms.patch /tmp/aquamarine-fullscreen.patch /tmp/aquamarine-shm.patch /tmp/hyprland-shm.patch
COPY --from=omarchy-cma /usr/local/bin/ /usr/local/bin/
# Toolchain donor stays pinned; the supervisor must match this source revision.
COPY --from=build /out/layer4-runtime /usr/local/bin/layer4-runtime
COPY --chmod=755 api/crates/layer4-runtime/src/browser/command.cjs /usr/local/bin/cma-browser
COPY --from=omarchy-cma /usr/local/lib/ /usr/local/lib/
COPY --from=omarchy-cma /etc/codex/ /etc/codex/
COPY --from=omarchy-cma /usr/share/cma-runtime-home/ /usr/share/cma-runtime-home/
COPY --from=omarchy-cma /usr/share/cma-desktop/ /usr/share/cma-desktop/
COPY api/crates/layer4-runtime/src/desktop-browser.py /usr/local/bin/cma-desktop-browser
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/session.sh /usr/local/bin/cma-desktop-session
COPY --chmod=0755 api/crates/layer4-runtime/src/omarchy/inner-session.sh /usr/local/bin/cma-omarchy-inner-session
COPY api/crates/layer4-runtime/src/omarchy/hyprland.lua /usr/share/cma-omarchy/hyprland.lua
COPY api/crates/layer4-runtime/src/omarchy/graphics-device.py /usr/share/cma-omarchy/graphics-device.py
COPY api/crates/layer4-runtime/src/omarchy/chromium.desktop /usr/share/applications/chromium.desktop
RUN groupadd --gid 10001 cma \
    && useradd --create-home --uid 10001 --gid 10001 --shell /bin/bash cma \
    && cp -a /usr/share/cma-runtime-home/. /home/cma/ \
    && mkdir -p /var/lib/cma-runtime /var/lib/cma-transfer \
    && chown -R 10001:10001 /home/cma /var/lib/cma-runtime /var/lib/cma-transfer \
    && codex --version && node --version && nyxid --version && gh --version
WORKDIR /home/cma
USER 10001:10001
ENV HOME=/home/cma CHRONO_RUNTIME_GENERATION=1 CHRONO_RUNTIME_HOME_IDENTITY="" \
    PATH=/home/cma/.local/bin:/usr/local/bin:/usr/share/omarchy/bin:/usr/bin \
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

FROM runtime AS default-runtime
ARG CHRONO_BUILD_SHA=local-unpinned
ARG CHRONO_BUILD=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}"
