#!/bin/bash
# Build only the CLI against the stable mirror's distributed shared libraries.
set -euo pipefail
export LC_ALL=C
output=$1
benchmark_patch=$2
source_version=3.6.5
package_version="$source_version-1"
if [ "$(pacman -Q openssl)" != "openssl $package_version" ]; then
    printf 'Arch OpenSSL package must match reviewed source %s\n' "$package_version" >&2
    exit 1
fi

# Use the flags the Arch package build applies outside its PKGBUILD.
source /etc/makepkg.conf
test "$CARCH" = x86_64
export CC=gcc CFLAGS="$CFLAGS $LTOFLAGS" CPPFLAGS="${CPPFLAGS:-}" LDFLAGS
read -r -a preprocessor_flags <<< "$CPPFLAGS $CFLAGS"
fortify=$(printf '#include <features.h>\n' | "$CC" "${preprocessor_flags[@]}" -dM -E -x c -)
grep -Fxq '#define __USE_FORTIFY_LEVEL 3' <<< "$fortify"

verify_hardening() {
    local executable=$1 headers dynamic imports
    headers=$(readelf -lW "$executable")
    dynamic=$(readelf -dW "$executable")
    imports=$(readelf --dyn-syms -W "$executable")
    if ! grep -qw GNU_RELRO <<< "$headers" || ! grep -qw BIND_NOW <<< "$dynamic" ||
        ! grep -Eq '\(FLAGS_1\).*\bPIE\b' <<< "$dynamic" ||
        ! grep -Eq 'UND[[:space:]]+__stack_chk_fail@' <<< "$imports" ||
        ! grep -Eq 'UND[[:space:]]+__[[:alnum:]_]+_chk@' <<< "$imports"; then
        printf 'OpenSSL CLI hardening check failed for %s\n' "$executable" >&2
        exit 1
    fi
    printf 'OpenSSL CLI retains full RELRO, PIE, stack protection and fortified imports: %s\n' "$executable"
}

verify_hardening /usr/bin/openssl

source_dir=$(mktemp -d)
trap 'rm -rf "$source_dir"' EXIT
cd "$source_dir"
archive="openssl-$source_version.tar.gz"
curl -fsSL --connect-timeout 15 --max-time 120 --retry 2 \
    "https://github.com/openssl/openssl/releases/download/openssl-$source_version/$archive" -o "$archive"
printf '%s  %s\n' a2157c2830efdec3788939b00c9b0638306d3f0bbb76dc4832ee503bb397df98 "$archive" | sha256sum -c -
tar -xzf "$archive"
cd "openssl-$source_version"
patch --batch --forward --fuzz=0 -p1 < "$benchmark_patch"
mkdir build
cd build
../Configure linux-x86_64 shared --prefix=/usr --openssldir=/etc/ssl --libdir=lib \
    --release no-tests enable-ktls enable-brotli enable-zlib enable-zstd enable-ec_nistp_64_gcc_128
for library in libcrypto libssl; do
    ln -s "/usr/lib/$library.so" "$library.so"
done
jobs=$(nproc)
make -s -j"$jobs" build_generated
make -s -j"$jobs" -o libcrypto.so -o libssl.so apps/openssl
binary=apps/openssl
stock=$(/usr/bin/openssl list -commands)
rebuilt=$("$binary" list -commands)
stock=$(printf '%s\n' "$stock" | tr -s '[:space:]' '\n' | sed '/^$/d;/^speed$/d' | sort)
rebuilt=$(printf '%s\n' "$rebuilt" | tr -s '[:space:]' '\n' | sed '/^$/d' | sort)
if [ "$stock" != "$rebuilt" ]; then
    echo "Rebuilt OpenSSL command set differs from the packaged CLI" >&2
    exit 1
fi
"$binary" version
mkdir -p "$output"
install -m 0755 "$binary" "$output/openssl"
strip --strip-unneeded "$output/openssl"
verify_hardening "$output/openssl"
printf '%s\n' "$package_version" > "$output/version"
