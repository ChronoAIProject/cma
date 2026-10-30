#!/bin/bash
# Rebuild the two bundled Go utilities with patched, checksum-pinned inputs.
set -euo pipefail
build_dir=$(mktemp -d /tmp/cma-omarchy-tools.XXXXXX)
trap 'rm -rf "$build_dir"' EXIT
cd "$build_dir"
curl -fsSL https://go.dev/dl/go1.26.10.linux-amd64.tar.gz -o go.tar.gz
echo 'ccfe7ae822d2c2f41f4fe7c956199967a4dac22d0ee0d2b2bfdb440aab3b20c5  go.tar.gz' | sha256sum -c -
tar xzf go.tar.gz
export PATH="$build_dir/go/bin:$PATH" GOTOOLCHAIN=local CGO_ENABLED=0
export GOPATH="$build_dir/modules" GOCACHE="$build_dir/cache"
curl -fsSL https://github.com/charmbracelet/gum/archive/879f048103adf0214b85943b52d8d65b08d772c5.tar.gz -o gum.tar.gz
echo '97a53dc52dd667bb6124051fe0e44bfe29740f1c9423eae40a326e17ba794ddd  gum.tar.gz' | sha256sum -c -
mkdir gum
tar xzf gum.tar.gz -C gum --strip-components=1
cd gum
go mod edit -require=golang.org/x/net@v0.60.0 -require=golang.org/x/text@v0.42.0
go mod tidy
go mod download all
go mod verify
test "$(go list -m -f '{{.Version}}' golang.org/x/net)" = v0.60.0
test "$(go list -m -f '{{.Version}}' golang.org/x/text)" = v0.42.0
go build -mod=readonly -trimpath -ldflags='-s -w -X main.Version=2.0.2' -o /usr/bin/gum .
gum format 'CMA runtime utility check' >/dev/null
cd "$build_dir"
curl -fsSL https://mirrors.edge.kernel.org/pub/linux/libs/security/linux-privs/libcap2/libcap-2.78.tar.xz -o libcap.tar.xz
echo '0d621e562fd932ccf67b9660fb018e468a683d7b827541df27813228c996bb11  libcap.tar.xz' | sha256sum -c -
tar xf libcap.tar.xz
cd libcap-2.78/goapps/captree
go mod edit -replace=kernel.org/pub/linux/libs/security/libcap/cap=../../cap \
  -replace=kernel.org/pub/linux/libs/security/libcap/psx=../../psx
go mod tidy
go build -mod=readonly -trimpath -ldflags='-s -w' -o /usr/bin/captree .
captree "$$" >/dev/null
go version -m /usr/bin/gum /usr/bin/captree > /usr/share/cma-omarchy/patched-tools.txt
sha256sum /usr/bin/gum /usr/bin/captree >> /usr/share/cma-omarchy/patched-tools.txt
cd /
