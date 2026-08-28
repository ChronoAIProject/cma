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

ARG CHRONO_BUILD_SHA=local-unpinned
LABEL org.opencontainers.image.revision="${CHRONO_BUILD_SHA}"

ARG NYXID_CLI_VERSION=v0.10.0
ARG CODEX_VERSION=0.147.0
ARG CODEX_NATIVE_SHA256_AMD64=cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40
ARG CODEX_NATIVE_SHA256_ARM64=e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2
# The kernel's Agent Client Protocol bridge. A conversation-mode session runs
# this instead of the terminal application: same kernel, driven through a
# program that speaks the contract rather than painting a screen.
#
# Pinned exactly, like everything else here. It declares @openai/codex ^0.147.0
# as its own dependency, so the two move together rather than fighting; a
# floating version could pull a bridge built against a kernel this image does
# not carry.
ARG CODEX_ACP_VERSION=1.4.0
# The second kernel, and its Agent Client Protocol bridge. One package rather
# than two: the bridge depends on @anthropic-ai/claude-agent-sdk, which ships
# the kernel it drives, so installing the bridge installs a matched pair. The
# Zed-maintained adapter was chosen over writing one against the SDK directly —
# it is Apache-2.0, it already maps prompts, session updates and permission
# requests onto the protocol this milestone adopted, and a bridge we wrote
# ourselves would be a second implementation of that mapping to keep correct.
#
# Pinned exactly, like everything else here, and for the sharper reason: this
# bridge decides how much a session's agent may do unattended, and a floating
# version could rename a mode the adapter sends by name.
ARG CLAUDE_CODE_ACP_VERSION=0.16.2
# The third kernel, and the JSON-RPC server its harness serves out of process.
# Two packages because they are two things: the harness, and the plugin that
# exposes it over stdio. Their version lines are separate upstream and both are
# pinned exactly — every package in this harness is a release candidate with a
# declared breaking-change policy, so a floating range is a different agent.
ARG DSH_VERSION=0.1.0-rc.6
ARG DSH_SDK_JSONRPC_VERSION=0.1.0-rc.6
# Node itself, because the harness above needs one newer than this base ships.
#
# Taken from nodejs.org and checked against a digest recorded here rather than
# one fetched alongside it: a checksum from the same host over the same
# connection proves the download was not corrupted, and nothing more. These two
# were read once, by hand, and a tarball that no longer matches fails the build
# instead of shipping.
ARG NODE_VERSION=24.19.0
ARG NODE_SHA256_AMD64=14b342e71204f811bde6153be8e04b62aef63c236fef92b55f9c83154b409647
ARG NODE_SHA256_ARM64=01443c1e1a29e531ccad5a46fefa6df490d2189c49f7955904aecdbb0fe86fdc
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
# Codex uses the first bwrap on PATH. The distro path lets a Linux host apply
# its targeted user-namespace AppArmor profile; without it Codex falls back to
# an anonymous bundled helper that restricted hosts reject.
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
        bash \
        bubblewrap \
        ca-certificates \
        curl \
        git \
        # The platform CLI links libdbus for OS keyring access. It never uses
        # the keyring here (its token comes from the environment), but the
        # library must be present for the binary to load at all.
        libdbus-1-3 \
        ncurses-term \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Node from upstream rather than from apt. The harness this image carries needs
# a runtime newer than the base distribution ships, and the two agents already
# here run on it: one declares node >=16, the other >=18.
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

# The package manager the Node tarball bundles carries published advisories of
# its own — six at the version shipped with this release, in four of its own
# vendored dependencies. It is not this image's to choose, and it is not
# removable either: a coding agent working in a sandbox on a Node project needs
# one.
#
# So it is upgraded to a pinned newer npm, which fixes most of them, and the two
# its bundle still carries are replaced the way any other pinned dependency is —
# the patched release of the same major, fetched into a scratch directory and
# moved into place so nothing else in that tree moves.
#
# This block is maintenance and shrinks as npm moves. The scan is what says when
# a line in it has become redundant, and the gate is what stops it being skipped
# instead.
ARG NPM_VERSION=12.0.2
ARG BRACE_EXPANSION_VERSION=5.0.9
ARG IP_ADDRESS_VERSION=10.3.1
RUN set -eux; \
    npm install -g "npm@${NPM_VERSION}"; \
    bundled=/usr/local/lib/node_modules/npm/node_modules; \
    npm install --no-save --prefix /tmp/npmfix \
        "brace-expansion@${BRACE_EXPANSION_VERSION}" \
        "ip-address@${IP_ADDRESS_VERSION}"; \
    for pkg in brace-expansion ip-address; do \
      rm -rf "$bundled/$pkg"; \
      mv "/tmp/npmfix/node_modules/$pkg" "$bundled/$pkg"; \
    done; \
    rm -rf /tmp/npmfix; \
    npm cache clean --force; \
    npm --version

# The second bridge pins a version of minimatch with two published
# denial-of-service advisories, and pins it exactly — so npm will not move it and
# a newer bridge is not available that does. The patched release of the same
# major is installed over it below.
#
# Stated rather than allowlisted, which is the point: the scan gate is what
# noticed, the gate is never weakened, and this is the remediation the gate was
# asking for. It comes out again the moment upstream pins a fixed version, and
# the scan is what will say so.
#
# Fetched into a scratch directory and moved into place, rather than installed
# in the bridge's own: npm reads the manifest it finds there and would pull that
# package's development dependencies into a runtime image — which is how a test
# runner with a critical advisory arrived in a sandbox the first time this was
# written. Moving one directory changes one directory.
ARG MINIMATCH_VERSION=10.2.3
# The third kernel runs the agent's shell tools through node-pty, which
# publishes no prebuild for this runtime and must be compiled. The toolchain is
# installed and removed inside one layer on purpose: a compiler left in a
# sandbox image is both a larger vulnerability surface for the scan gate and a
# capability handed to whoever is driving the agent.
#
# codex is run afterwards because this is where the Node under it changed, and
# "the agent still starts" is the one claim that has to be checked rather than
# reasoned about.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends build-essential python3; \
    npm install -g "@openai/codex@${CODEX_VERSION}" \
        "@agentclientprotocol/codex-acp@${CODEX_ACP_VERSION}" \
        "@zed-industries/claude-code-acp@${CLAUDE_CODE_ACP_VERSION}"; \
    npm install --no-save --prefix /tmp/patched "minimatch@${MINIMATCH_VERSION}"; \
    rm -rf /usr/local/lib/node_modules/@zed-industries/claude-code-acp/node_modules/minimatch; \
    mv /tmp/patched/node_modules/minimatch \
        /usr/local/lib/node_modules/@zed-industries/claude-code-acp/node_modules/minimatch; \
    rm -rf /tmp/patched; \
    npm cache clean --force; \
    test "$(node -p "require('/usr/local/lib/node_modules/@openai/codex/package.json').version")" = "${CODEX_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "${CODEX_ACP_VERSION}"; \
    test "$(codex --version)" = "codex-cli ${CODEX_VERSION}"; \
    case "${TARGETARCH}" in \
      amd64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_AMD64}" ;; \
      arm64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_ARM64}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    echo "${want}  ${native}" | sha256sum -c -

# `codex-acp` exposes no bounded unattended mode at this exact pin: its agent
# mode asks for approval and disables network, while its unattended mode also
# disables filesystem confinement. Trigger sessions need the missing middle:
# workspace-write, no human approval, and managed networking whose allowlist is
# supplied by the signed Trigger-only Codex profile. Patch only the exact known
# compiled shape and fail the build when the pinned package moves underneath
# us, so an upstream refactor cannot silently widen or disable this boundary.
RUN node <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = "/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js";
const source = fs.readFileSync(path, "utf8");
const codexPackage = JSON.parse(fs.readFileSync(
  "/usr/local/lib/node_modules/@openai/codex/package.json",
  "utf8"
));
const bridgePackage = JSON.parse(fs.readFileSync(
  "/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json",
  "utf8"
));
if (codexPackage.version !== "0.147.0" || bridgePackage.version !== "1.4.0") {
  throw new Error("the CMA Trigger permission patch only supports Codex 0.147.0 through codex-acp 1.4.0");
}
if (bridgePackage.dependencies?.["@openai/codex"] !== "^0.147.0") {
  throw new Error("codex-acp 1.4.0 no longer declares the reviewed Codex protocol line");
}
function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}
if (sha256(source) !== "ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf") {
  throw new Error("codex-acp 1.4.0 compiled source differs from the reviewed artifact");
}
const codexLauncher = fs.readFileSync(
  "/usr/local/lib/node_modules/@openai/codex/bin/codex.js"
);
if (sha256(codexLauncher) !== "134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477") {
  throw new Error("Codex 0.147.0 launcher differs from the reviewed artifact");
}
const agent = `  static Agent = new _AgentMode(
    "agent",
    "Agent",
    "Read and edit files, and run commands.",
    "on-request",
    {
      type: "workspaceWrite",
      writableRoots: [],
      networkAccess: false,
      excludeTmpdirEnvVar: false,
      excludeSlashTmp: false
    },
    "workspace-write"
  );`;
const trigger = `
  static CmaTrigger = new _AgentMode(
    "cma-trigger",
    "CMA Trigger",
    "Edit files unattended with managed loopback network access.",
    "never",
    null,
    "permission-profile:cma-trigger"
  );`;
const modes = `    return [_AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];`;
const patchedModes = `    if (process.env["INITIAL_AGENT_MODE"] === "cma-trigger") {
      return [_AgentMode.CmaTrigger];
    }
    return [_AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];`;
const sessionConfig = `  async createSessionConfig(projectPath, additionalDirectories, mcpServers) {
    const sessionRoots = [projectPath, ...additionalDirectories];`;
const patchedSessionConfig = `  async createSessionConfig(projectPath, additionalDirectories, mcpServers) {
    if (process.env["INITIAL_AGENT_MODE"] === "cma-trigger" && additionalDirectories.length !== 0) {
      throw new Error("cma-trigger does not accept additional workspace directories");
    }
    const sessionRoots = [projectPath, ...additionalDirectories];`;
const promptTurn = `    return await this.codexClient.runTurn({
      threadId: request.sessionId,
      input,
      approvalPolicy: agentMode.approvalPolicy,
      sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories),
      summary: disableSummary ? "none" : "auto",`;
const patchedPromptTurn = `    const turnPermissions = agentMode.id === "cma-trigger" ? {
      permissions: "cma-trigger"
    } : {
      sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories)
    };
    return await this.codexClient.runTurn({
      threadId: request.sessionId,
      input,
      approvalPolicy: agentMode.approvalPolicy,
      ...turnPermissions,
      summary: disableSummary ? "none" : "auto",`;
function requireExactlyOnce(haystack, needle, label) {
  const first = haystack.indexOf(needle);
  if (first < 0 || first !== haystack.lastIndexOf(needle)) {
    throw new Error(`codex-acp 1.4.0 ${label} source shape drifted`);
  }
}
requireExactlyOnce(source, agent, "agent mode");
requireExactlyOnce(source, modes, "mode list");
requireExactlyOnce(source, sessionConfig, "session workspace configuration");
requireExactlyOnce(source, promptTurn, "prompt turn sandbox policy");
if (source.includes('"cma-trigger"')) {
  throw new Error("codex-acp already contains a cma-trigger mode");
}
const patched = source
  .replace(agent, agent + trigger)
  .replace(modes, patchedModes)
  .replace(sessionConfig, patchedSessionConfig)
  .replace(promptTurn, patchedPromptTurn);
requireExactlyOnce(patched, trigger, "patched trigger mode");
requireExactlyOnce(patched, patchedModes, "patched mode list");
requireExactlyOnce(patched, patchedSessionConfig, "patched session workspace guard");
requireExactlyOnce(patched, patchedPromptTurn, "patched profile-native prompt turn");
requireExactlyOnce(
  patched,
  `return [_AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];`,
  "ordinary mode vocabulary"
);
if (sha256(patched) !== "2c370f2337a5cc689d9a76b4bba1289d42afe6c17de824e2ca48004ba9670ab6") {
  throw new Error("patched codex-acp 1.4.0 source differs from the reviewed artifact");
}
fs.writeFileSync(path, patched);
NODE

# The third kernel gets its own tree rather than a global install, because it
# needs something a global install cannot express.
#
# Its closure is two hundred packages deep and carries advisories of its own —
# seven at this pin, across four transitive dependencies. The gate is never
# weakened, and there is no newer harness that resolves them, so each is pinned
# forward to the patched release of the same major. `overrides` is the tool for
# exactly that, and it only works inside a project, which is what this
# directory is. A resolution that cannot satisfy one fails the build rather
# than shipping around it.
#
# This list is maintenance, and is meant to be: it shrinks as upstream moves,
# and the scan is what says when an entry has become redundant.
ARG DSH_VERSION
COPY <<JSON /opt/dsh-install/package.json
{
  "name": "chrono-dsh-install",
  "private": true,
  "dependencies": { "@deepseek-ai/dsh": "${DSH_VERSION}" },
  "overrides": {
    "tar": "7.5.19",
    "undici": "6.27.0",
    "brace-expansion": "5.0.9",
    "ip-address": "10.3.1"
  }
}
JSON
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends build-essential python3; \
    NODE_OPTIONS=--max-old-space-size=4096 \
        npm install --prefix /opt/dsh-install --no-audit --no-fund; \
    npm cache clean --force; \
    apt-get purge -y --auto-remove build-essential python3; \
    rm -rf /var/lib/apt/lists/*; \
    ln -s /opt/dsh-install/node_modules/.bin/dsh /usr/local/bin/dsh; \
    dsh --version

# npm and the harness both carry node-tar 7.5.19, which is vulnerable to
# CVE-2026-73566. Keep their known-good dependency resolutions intact, then
# replace only that package with the fixed release. Both trees already carry
# the same dependency shape required by 7.5.21.
ARG TAR_VERSION=7.5.21
RUN set -eux; \
    npm install --no-save --prefix /tmp/tarfix "tar@${TAR_VERSION}"; \
    for target in \
        /usr/local/lib/node_modules/npm/node_modules/tar \
        /opt/dsh-install/node_modules/tar; do \
      rm -rf "$target"; \
      cp -a /tmp/tarfix/node_modules/tar "$target"; \
      test "$(node -p "require('$target/package.json').version")" = "${TAR_VERSION}"; \
    done; \
    rm -rf /tmp/tarfix; \
    npm cache clean --force

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

# The harness boots a profile — an ordered stack of plugin layers — and the one
# it ships expects a workstation: telemetry to a vendor endpoint, hot reload
# watching the working directory, and a model route to the vendor's own API. A
# session here is none of those things, so the profile is ours and is baked in
# rather than written at startup, where a session could edit it.
#
# What it changes, and why each one:
#
#   sdk-jsonrpc-server  mounted, because it is the whole reason this kernel is
#                       reachable: it serves the harness over stdio, which is
#                       what the supervisor speaks to a bridge.
#   llm-pi-ai           one route, at the supervisor's loopback proxy with the
#                       inert placeholder key. The real credential never enters
#                       the sandbox; the proxy attaches it on the way out.
#   agent-default-model that route, so a session that names no model still has
#                       one and does not reach for the vendor's default.
#   telemetry, hmr      off. One would send a tenant's session activity to a
#                       third party; the other watches a working directory the
#                       agent is actively editing.
#
# The !!js expressions are the profile format's own and are read at boot, so one
# image serves every deployment: what the adapter puts in the environment
# decides the route and the model.
#
# It lives under the pod's home rather than somewhere system-wide, because the
# launcher writes into the profile directory on every boot and a sandbox runs
# against a read-only root filesystem. The home is the one place it may write,
# and it is where this kernel keeps its conversations anyway.
RUN useradd --create-home --uid 10001 --shell /bin/bash chrono \
    && mkdir -p /home/chrono/.dsh/profiles/chrono \
    && chown -R 10001:10001 /home/chrono/.dsh
COPY --chown=10001:10001 <<'JSON' /home/chrono/.dsh/profiles/chrono/package.json
{
  "name": "dsh-profile-chrono",
  "private": true,
  "dsh": {
    "profile": {
      "bundles": [
        "@deepseek-ai/dsh-base"
      ]
    }
  }
}
JSON
COPY --chown=10001:10001 <<'YAML' /home/chrono/.dsh/profiles/chrono/cordis.patch.yml
# Serves the harness over stdio, which is what the supervisor speaks to a
# bridge. Nothing else in the tree exposes it.
- insert:
    - id: sdk-jsonrpc-server
      name: '@deepseek-ai/dsh-sdk-jsonrpc-server'

# One model route, at the supervisor's loopback proxy. The key named here is a
# placeholder the proxy replaces outright: the real credential never enters the
# sandbox, and an agent that could read it is an agent whose owner's credential
# is reachable by anyone driving the session.
- id: llm-pi-ai
  config:
    providers:
      chrono:
        displayName: Chrono
        api: !!js process.env.CHRONO_DSH_WIRE_API || 'openai-completions'
        baseURL: !!js process.env.CHRONO_DSH_PROXY_URL || 'http://127.0.0.1:9091'
        apiKeyEnv: CHRONO_UPSTREAM_PLACEHOLDER_KEY
        models:
          - id: !!js process.env.CHRONO_UPSTREAM_MODEL || 'deepseek-v4-flash'
            name: !!js process.env.CHRONO_UPSTREAM_MODEL || 'deepseek-v4-flash'

# So a session that names no model still runs on the route above rather than
# reaching for the vendor's own default, which this sandbox cannot reach.
- id: agent-default-model
  config:
    provider: chrono
    model: !!js process.env.CHRONO_UPSTREAM_MODEL || 'deepseek-v4-flash'

# A tenant's session activity is not a third party's to receive.
- id: session-telemetry-otel
  disabled: true

# Watches the working directory the agent is actively editing.
- id: hmr
  disabled: true
YAML

COPY --from=build /build/target/release/layer4-runtime /usr/local/bin/layer4-runtime
USER 10001:10001
ENV HOME=/home/chrono \
    DSH_HOME=/home/chrono/.dsh

# The plugin that serves this harness over stdio is not one of its bundles, so
# it installs beside the profile that names it — the out-of-tree plugin path the
# launcher documents.
#
# Its peers come with it because one of them, the protocol package, is in no
# bundle either and nothing else would supply it. The ones the installation does
# carry are then removed again, so the parent walk reaches the launcher's flat
# fallback and the plugin ends up holding the same instances the booted tree
# does. A second copy of the session or agent package would be a plugin
# subscribing to events nothing publishes.
ARG DSH_SDK_JSONRPC_VERSION
RUN set -eux; \
    profile=/home/chrono/.dsh/profiles/chrono; \
    npm install --prefix "$profile" --no-save \
        "@deepseek-ai/dsh-sdk-jsonrpc-server@${DSH_SDK_JSONRPC_VERSION}"; \
    npm cache clean --force; \
    for dep in "$profile"/node_modules/@deepseek-ai/*; do \
      name=$(basename "$dep"); \
      if [ "$name" != "dsh-sdk-jsonrpc-server" ] \
         && [ -e "/opt/dsh-install/node_modules/@deepseek-ai/$name" ]; then \
        rm -rf "$dep"; \
      fi; \
    done

# Compose and boot the profile once, for two reasons at once.
#
# It is a build-time check that the layer above actually works: a patch naming
# an entry the bundle does not have is a warning at boot, and a warning at boot
# is a session that quietly runs on the vendor's defaults instead of ours. The
# boot goes further than the dump — it loads every plugin the profile names
# rather than composing the list of them.
#
# And it leaves the launcher's flat module fallback populated, which is what
# makes the plugin above resolve its peers to the tree's own instances. The dump
# comes first because that is the step which heals it, and the plugin cannot
# load until it has.
#
# The boot is ended by asking it something rather than by closing its input:
# this harness's own bin owns its exit, and a tree with nothing to do keeps
# running. Requiring an answer is the stronger check anyway — it proves the
# plugin is serving, not merely that the modules resolved — and the pipe closing
# on the match is what stops it.
RUN set -eux; \
    dsh --profile chrono --dump-config > /dev/null; \
    test -d /home/chrono/.dsh/profiles/node_modules; \
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"cwd":"/home/chrono","provider":"chrono","model":"probe"}}' \
      | timeout 180 dsh --profile chrono \
      | grep -q '"serverInfo"'

# Refresh the final filesystem after all expensive runtime layers are cached.
# CI and release builds set this to their workflow attempt, so a newly fixed
# Debian package cannot remain hidden behind an older successful image cache.
ARG OS_SECURITY_REFRESH=local
USER root
RUN test -n "${OS_SECURITY_REFRESH}" \
    && apt-get update \
    && apt-get upgrade -y \
    && rm -rf /var/lib/apt/lists/*
USER 10001:10001
# CHRONO_RUNTIME_SNAPSHOT_DIR is where the kernel below records its
# conversation, relative to HOME. The supervisor is kernel-neutral and does not
# know this; the adapter that starts a sandbox sets it explicitly, and the value
# here is only what the image runs with when something starts it by hand.
#
# Left unset it would mean the runtime declares no snapshot support, and a
# session on it could never be carried onto a newer image — so the default
# matches the kernel this image is actually built with rather than being empty.
#
# CHRONO_RUNTIME_SNAPSHOT_NAMED says the kernel below has to be told *which*
# conversation to continue. Asked for "the latest" it selects among
# conversations recorded in the current working directory, and one carried from
# another pod was recorded somewhere else — so it finds nothing, starts fresh,
# and reports success.
ENV CHRONO_RUNTIME_BIND=0.0.0.0:9090 \
    CHRONO_RUNTIME_COMMAND=codex \
    CHRONO_RUNTIME_SNAPSHOT_DIR=.codex/sessions \
    CHRONO_RUNTIME_SNAPSHOT_NAMED=true \
    TERM=xterm-256color
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/layer4-runtime"]
