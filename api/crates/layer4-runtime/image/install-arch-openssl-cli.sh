#!/bin/sh
# Replace the CLI inside the package producer, preserving distributor libraries.
set -eu
artifact=$1
root=$2
installed=$(pacman --root "$root" -Q openssl)
rebuilt=$(cat "$artifact/version")
if [ -z "$rebuilt" ] || [ "$installed" != "openssl $rebuilt" ]; then
    printf 'OpenSSL CLI package version mismatch: installed=%s rebuilt=%s\n' "$installed" "$rebuilt" >&2
    exit 1
fi
install -m 0755 "$artifact/openssl" "$root/usr/bin/openssl"
