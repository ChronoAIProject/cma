# syntax=docker/dockerfile:1
# CMA_MODEL_PROXY_PLACEHOLDER is supplied only in managed Agent launch plans,
# not in the Sandbox environment, so existing runtime identities stay stable.
# Bot-pod image for a contract-3 CMA Sandbox.
#
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
    cargo build --release --locked --bin layer4-runtime \
    && mkdir /out \
    && cp /build/target/release/layer4-runtime /out/layer4-runtime

# The NyxID CLI is published as a *-unknown-linux-gnu binary.
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS runtime

ARG NYXID_CLI_VERSION=v0.10.0
ARG CODEX_VERSION=0.147.0
ARG CODEX_NATIVE_SHA256_AMD64=cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40
ARG CODEX_NATIVE_SHA256_ARM64=e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2
ARG CODEX_LAUNCHER_SHA256=134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477
ARG CODEX_ACP_VERSION=1.4.0
ARG CODEX_ACP_SHA256=ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf

# Node is downloaded from upstream and verified against digests recorded here.
# Both Codex packages are JavaScript launchers for pinned native/runtime code.
ARG NODE_VERSION=24.19.0
ARG NODE_SHA256_AMD64=14b342e71204f811bde6153be8e04b62aef63c236fef92b55f9c83154b409647
ARG NODE_SHA256_ARM64=01443c1e1a29e531ccad5a46fefa6df490d2189c49f7955904aecdbb0fe86fdc
ARG TARGETARCH

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
ARG BRACE_EXPANSION_VERSION=5.0.9
ARG IP_ADDRESS_VERSION=10.3.1
ARG TAR_VERSION=7.5.21
RUN set -eux; \
    npm install -g "npm@${NPM_VERSION}"; \
    bundled=/usr/local/lib/node_modules/npm/node_modules; \
    npm install --no-save --prefix /tmp/npmfix \
        "brace-expansion@${BRACE_EXPANSION_VERSION}" \
        "ip-address@${IP_ADDRESS_VERSION}" \
        "tar@${TAR_VERSION}"; \
    for package in brace-expansion ip-address tar; do \
      rm -rf "$bundled/$package"; \
      cp -a "/tmp/npmfix/node_modules/$package" "$bundled/$package"; \
    done; \
    test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/tar/package.json').version")" = "${TAR_VERSION}"; \
    rm -rf /tmp/npmfix; \
    npm cache clean --force; \
    npm --version

# Install only the supported provider and its ACP bridge. Their package
# metadata, launcher, bridge source, and architecture-specific native binary
# are all checked against the reviewed pinned artifacts.
RUN set -eux; \
    npm install -g "@openai/codex@${CODEX_VERSION}" \
        "@agentclientprotocol/codex-acp@${CODEX_ACP_VERSION}"; \
    npm cache clean --force; \
    test "$(node -p "require('/usr/local/lib/node_modules/@openai/codex/package.json').version")" = "${CODEX_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "${CODEX_ACP_VERSION}"; \
    test "$(codex --version)" = "codex-cli ${CODEX_VERSION}"; \
    echo "${CODEX_LAUNCHER_SHA256}  /usr/local/lib/node_modules/@openai/codex/bin/codex.js" | sha256sum -c -; \
    echo "${CODEX_ACP_SHA256}  /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js" | sha256sum -c -; \
    case "${TARGETARCH}" in \
      amd64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_AMD64}" ;; \
      arm64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_ARM64}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    echo "${want}  ${native}" | sha256sum -c -

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

# Baseline skills are part of the runtime contract, so their source revisions
# and source archives are pinned just like executable dependencies. Additional
# skills are materialized per agent by the supervisor.
ARG NYXID_SKILL_COMMIT=0ac096837c7436b05b71b374207a42320776715d
ARG NYXID_SKILL_ARCHIVE_SHA256=6559072f4d4ab35b466cdf64ae0ae86ee3fb89a36f67925ed1ba4eb781ee23b8
ARG ORNN_SKILL_COMMIT=052024a6927306f2455843b850a948d77ac67f9d
ARG ORNN_SKILL_ARCHIVE_SHA256=a5788101f78d364e14b57bfc2fa1755702833b239b280ed6ce60c5abd3a442c3
RUN set -eux; \
    groupadd --gid 10001 cma; \
    useradd --create-home --uid 10001 --gid 10001 --shell /bin/bash cma; \
    install -d -o 10001 -g 10001 \
        /home/cma/.cma/agents \
        /home/cma/.cma/provider/codex \
        /home/cma/.cma/checkpoints \
        /home/cma/.codex/skills \
        /var/lib/cma-runtime; \
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
    chown -R 10001:10001 /home/cma /var/lib/cma-runtime; \
    rm -rf /tmp/baseline-skills /tmp/nyxid-source.tar.gz /tmp/ornn-source.tar.gz

COPY --from=build /out/layer4-runtime /usr/local/bin/layer4-runtime

# Refresh the final filesystem after expensive dependency layers are cached.
# CI and release builds vary this value so cached OS packages cannot hide a
# newly published security update from the image scan.
ARG OS_SECURITY_REFRESH=local
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apt-get update \
    && apt-get upgrade -y \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /home/cma
USER 10001:10001
ENV HOME=/home/cma \
    CHRONO_RUNTIME_BIND=0.0.0.0:9090 \
    TERM=xterm-256color
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]

ARG CHRONO_BUILD_SHA=local-unpinned
ARG CHRONO_BUILD=development
LABEL org.opencontainers.image.source="https://github.com/ChronoAIProject/cma" \
      org.opencontainers.image.revision="${CHRONO_BUILD_SHA}" \
      org.opencontainers.image.version="${CHRONO_BUILD}" \
      dev.chrono.cma.runtime-contract="3"
