#!/bin/sh
# Frozen shallow installs retain the native paths covered by provider checks.
set -eu
# Codex 0.165 disables the existing checklist tool unless explicitly configured.
mkdir -p /etc/codex
printf '[tools.update_plan]\nenabled = true\n' > /etc/codex/config.toml
if [ "${RUNTIME_DESKTOP}" = "1" ]; then
  CODEX_VERSION="${DESKTOP_CODEX_VERSION}"
  CODEX_NATIVE_SHA256_AMD64="${DESKTOP_CODEX_NATIVE_SHA256_AMD64}"
  CODEX_NATIVE_SHA256_ARM64="${DESKTOP_CODEX_NATIVE_SHA256_ARM64}"
  CODEX_LAUNCHER_SHA256="${DESKTOP_CODEX_LAUNCHER_SHA256}"
  printf '\n[features]\ncomputer_use = true\n' >> /etc/codex/config.toml
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
  amd64)
    target=x86_64-unknown-linux-musl
    platform=x64
    upstream="${CODEX_UPSTREAM_NATIVE_SHA256_AMD64}"
    package_sha="${CODEX_FORK_PACKAGE_SHA256_AMD64}"
    native_sha="${CODEX_NATIVE_SHA256_AMD64}"
    host_sha="${CODEX_FORK_CODE_MODE_SHA256_AMD64}"
    bwrap_sha="${CODEX_FORK_BWRAP_SHA256_AMD64}"
    rg_sha="${CODEX_FORK_RG_SHA256_AMD64}"
    ;;
  arm64)
    target=aarch64-unknown-linux-musl
    platform=arm64
    upstream="${CODEX_UPSTREAM_NATIVE_SHA256_ARM64}"
    package_sha="${CODEX_FORK_PACKAGE_SHA256_ARM64}"
    native_sha="${CODEX_NATIVE_SHA256_ARM64}"
    host_sha="${CODEX_FORK_CODE_MODE_SHA256_ARM64}"
    bwrap_sha="${CODEX_FORK_BWRAP_SHA256_ARM64}"
    rg_sha="${CODEX_FORK_RG_SHA256_ARM64}"
    ;;
  *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;;
esac
vendor="/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-${platform}/vendor/${target}"
echo "${upstream}  ${vendor}/bin/codex" | sha256sum -c -
# Keep the locked 0.165.0 launcher and vendor resources, and replace every
# executable in the reviewed native package with the corresponding fork build.
archive=/tmp/codex-package.tar.gz
curl -fsSL -o "$archive" "https://github.com/ChronoAIProject/codex/releases/download/${CODEX_FORK_RELEASE}/codex-package-${target}.tar.gz"
python3 /tmp/install-codex-package.py "$archive" "$package_sha" "$target" "$CODEX_VERSION" "$vendor" \
  "$native_sha" "$host_sha" "$bwrap_sha" "$rg_sha"
rm -f "$archive"
test "$(codex --version)" = "codex-cli ${CODEX_VERSION}"
