#!/bin/sh
# Build the CLI from the same signed Debian source as the runtime package.
set -eu
source_dir=$1
output=$2
benchmark_patch=$3
export LC_ALL=C DEBIAN_FRONTEND=noninteractive

sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/debian.sources
apt-get update
version=$(apt-cache policy openssl | awk '$1 == "Candidate:" {print $2}')
dpkg --validate-version "$version"
# Use the selected package's own shared libraries to build just the CLI.
apt-get install -y --no-install-recommends "libssl-dev=$version"
echo "Building the OpenSSL CLI from Debian source $version"
cd "$source_dir"
apt-get source --only-source "openssl=$version"
set -- openssl-*/
test "$#" -eq 1
cd "$1"
# The generated command table is derived from the build sources. Omitting
# speed also omits its RSA and DSA fixtures without altering the TLS library.
patch --batch --forward --fuzz=0 -p1 < "$benchmark_patch"
make -f debian/rules override_dh_auto_configure
library="/usr/lib/$(dpkg-architecture -qDEB_HOST_MULTIARCH)"
for name in libcrypto libssl; do
    ln -s "$library/$name.so" "build_shared/$name.so"
done
make -s -C build_shared -j"$(nproc)" build_generated
make -s -C build_shared -j"$(nproc)" -o libcrypto.so -o libssl.so apps/openssl
binary=build_shared/apps/openssl
commands=$(LD_LIBRARY_PATH="$PWD/build_shared" "$binary" list -commands)
for required in s_client s_server rehash fipsinstall; do
    printf '%s\n' "$commands" | grep -qw "$required"
done
if printf '%s\n' "$commands" | grep -qw speed; then
    echo "The rebuilt OpenSSL CLI still contains its benchmark command" >&2
    exit 1
fi
mkdir -p "$output"
install -m 0755 "$binary" "$output/openssl"
strip --strip-unneeded "$output/openssl"
printf '%s\n' "$version" > "$output/version"
rm -rf /var/lib/apt/lists/*
