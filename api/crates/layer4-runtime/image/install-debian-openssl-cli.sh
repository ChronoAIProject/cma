#!/bin/sh
# The system libraries and package database remain Debian's.
set -eu
artifact=$1
destination=$2
installed=$(dpkg-query -W -f='${Version}' openssl)
rebuilt=$(cat "$artifact/version")
if [ -z "$rebuilt" ] || [ "$rebuilt" != "$installed" ]; then
    printf 'OpenSSL CLI package version mismatch: installed=%s rebuilt=%s\n' "$installed" "$rebuilt" >&2
    exit 1
fi
install -m 0755 "$artifact/openssl" "$destination"
"$destination" version
