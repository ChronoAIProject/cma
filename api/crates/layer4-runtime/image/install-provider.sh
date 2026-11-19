#!/bin/sh
# Frozen shallow installs retain the native paths covered by provider checks.
set -eu
if [ "${RUNTIME_DESKTOP}" = "1" ]; then
  CODEX_VERSION="${DESKTOP_CODEX_VERSION}"
  CODEX_NATIVE_SHA256_AMD64="${DESKTOP_CODEX_NATIVE_SHA256_AMD64}"
  CODEX_NATIVE_SHA256_ARM64="${DESKTOP_CODEX_NATIVE_SHA256_ARM64}"
  CODEX_LAUNCHER_SHA256="${DESKTOP_CODEX_LAUNCHER_SHA256}"
  mkdir -p /etc/codex
  printf '[features]\ncomputer_use = true\n' > /etc/codex/config.toml
fi
inputs=/tmp/cma-image-inputs/provider
if [ "${RUNTIME_DESKTOP}" = "1" ]; then inputs=/tmp/cma-image-inputs/desktop-provider; fi
mkdir /tmp/provider-install
cp "$inputs/package.json" "$inputs/package-lock.json" /tmp/provider-install/
npm ci --prefix /tmp/provider-install --install-strategy=shallow --ignore-scripts --no-audit --no-fund
# The bridge imports Zod's published modules. Its upstream source tests carry
# example credentials and must stay outside the payload copied into the image.
zod_source=/tmp/provider-install/node_modules/@agentclientprotocol/codex-acp/node_modules/zod/src/v4
rm -rf "$zod_source/classic/tests" "$zod_source/mini/tests"
cp -a /tmp/provider-install/node_modules/@openai /usr/local/lib/node_modules/
cp -a /tmp/provider-install/node_modules/@agentclientprotocol /usr/local/lib/node_modules/
ln -s ../lib/node_modules/@openai/codex/bin/codex.js /usr/local/bin/codex
ln -s ../lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js /usr/local/bin/codex-acp
rm -rf /tmp/provider-install
npm cache clean --force
test "$(node -p "require('/usr/local/lib/node_modules/@openai/codex/package.json').version")" = "${CODEX_VERSION}"
test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "${CODEX_ACP_VERSION}"
test "$(codex --version)" = "codex-cli ${CODEX_VERSION}"
echo "${CODEX_LAUNCHER_SHA256}  /usr/local/lib/node_modules/@openai/codex/bin/codex.js" | sha256sum -c -
echo "${CODEX_ACP_UPSTREAM_SHA256}  /usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js" | sha256sum -c -
case "${TARGETARCH}" in
  amd64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_AMD64}" ;;
  arm64) native="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"; want="${CODEX_NATIVE_SHA256_ARM64}" ;;
  *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;;
esac
echo "${want}  ${native}" | sha256sum -c -
