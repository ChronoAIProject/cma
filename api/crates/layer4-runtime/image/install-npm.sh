#!/bin/sh
# Preserve the reviewed npm bundle patches using integrity-locked inputs.
set -eu
mkdir /tmp/npmfix
cp /tmp/cma-image-inputs/npm/package.json /tmp/cma-image-inputs/npm/package-lock.json /tmp/npmfix/
npm ci --prefix /tmp/npmfix --install-strategy=shallow --ignore-scripts --no-audit --no-fund
rm -rf /usr/local/lib/node_modules/npm
cp -a /tmp/npmfix/node_modules/npm /usr/local/lib/node_modules/
bundled=/usr/local/lib/node_modules/npm/node_modules
for package in brace-expansion ip-address tar undici; do
  rm -rf "$bundled/$package"
  cp -a "/tmp/npmfix/node_modules/$package" "$bundled/$package"
done
# Neither README is used by the npm CLI. Keep each package's separate license.
rm -f "$bundled/@npmcli/arborist/README.md" "$bundled/ip-address/README.md"
test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/tar/package.json').version")" = "${TAR_VERSION}"
test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/brace-expansion/package.json').version")" = "${BRACE_EXPANSION_VERSION}"
test "$(node -p "require('/usr/local/lib/node_modules/npm/node_modules/undici/package.json').version")" = "${UNDICI_VERSION}"
rm -rf /tmp/npmfix
npm cache clean --force
test "$(npm --version)" = "${NPM_VERSION}"
