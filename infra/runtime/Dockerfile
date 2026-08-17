# syntax=docker/dockerfile:1
# Bot-pod image: the sandboxed agent runtime.
#
# This image is never scheduled by the control plane directly. It is handed to
# the sandbox provider as the image for a sandbox, with the supervisor as its
# entrypoint; the provider isolates it and exposes its port back to us.
#
# It carries three things the control plane deliberately does not: the agent
# kernel, a real pseudo-terminal to run it under, and the platform CLI that
# lets a session use the capabilities its owner has connected.
FROM rust:1.96.1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY api/ ./
# Statically linked against musl, so it runs on any base the runtime stage uses.
RUN cargo build --release --locked --bin layer4-runtime

# glibc rather than Alpine: the platform CLI is published only as a
# *-unknown-linux-gnu binary, and a glibc binary will not run against musl.
FROM debian:trixie-slim AS runtime

ARG NYXID_CLI_VERSION=v0.10.0
ARG CODEX_VERSION=0.147.0
# Provided by buildx; selects the right published binary for the target.
ARG TARGETARCH

# ncurses-term is required, not cosmetic: a full-screen agent interface cannot
# address the cursor without a terminfo entry for TERM, and degrades into an
# unreadable flat scroll.
#
# The upgrade is what pulls security fixes published after the base image was
# cut, and is the counterpart of the apk upgrade in infra/api/Dockerfile. A
# Debian tag freezes its package set, so without this the image ships whatever
# was current when trixie-slim was built and never moves — which means the scan
# gate starts failing on a Dockerfile nobody has touched, the first time a
# vulnerability is published in anything the base image already carried.
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        # The platform CLI links libdbus for OS keyring access. It never uses
        # the keyring here (its token comes from the environment), but the
        # library must be present for the binary to load at all.
        libdbus-1-3 \
        ncurses-term \
        nodejs \
        npm \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force

# Install the platform CLI from its published release. The checksum is verified
# rather than trusted: this binary acts on the owner's behalf.
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) target="x86_64-unknown-linux-gnu" ;; \
      arm64) target="aarch64-unknown-linux-gnu" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    base="https://github.com/ChronoAIProject/NyxID/releases/download/${NYXID_CLI_VERSION}"; \
    curl -fsSL -o /tmp/nyxid.tar.gz "${base}/nyxid-cli-${target}.tar.gz"; \
    curl -fsSL -o /tmp/nyxid.sha256 "${base}/nyxid-cli-${target}.tar.gz.sha256"; \
    ( cd /tmp && awk '{print $1"  nyxid.tar.gz"}' nyxid.sha256 | sha256sum -c - ); \
    tar xzf /tmp/nyxid.tar.gz -C /tmp; \
    install -m 0755 "$(find /tmp -type f -name nyxid -perm -u+x | head -1)" /usr/local/bin/nyxid; \
    rm -rf /tmp/nyxid.tar.gz /tmp/nyxid.sha256; \
    nyxid --version

RUN useradd --create-home --uid 10001 --shell /bin/bash chrono
COPY --from=build /build/target/release/layer4-runtime /usr/local/bin/layer4-runtime
USER 10001:10001
ENV HOME=/home/chrono \
    CHRONO_RUNTIME_BIND=0.0.0.0:9090 \
    CHRONO_RUNTIME_COMMAND=codex \
    TERM=xterm-256color
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]
