#!/bin/bash
# Assemble an independent base so package omissions precede the first shipped layer.
set -euo pipefail
ulimit -c 0
export SOURCE_DATE_EPOCH=1
payload=$1
snapshot=$2
[[ $snapshot =~ ^[0-9]{4}/[0-9]{2}/[0-9]{2}$ ]]
# Every Arch and Omarchy package comes from the digest-pinned package bundle,
# mounted read-only. No package server is contacted.
bundle=/run/cma-omarchy-packages
test "$(cat "$bundle/SNAPSHOT")" = "$snapshot"
inputs=$(mktemp -d)
trap 'rm -rf "$inputs"' EXIT

verified_download() {
    local url=$1 digest=$2 target=$3
    curl -fsSL --connect-timeout 15 --max-time 120 --retry 2 "$url" -o "$target"
    printf '%s  %s\n' "$digest" "$target" | sha256sum -c -
}

cp /etc/pacman.conf "$inputs/arch-pacman.conf"
cp /etc/pacman.d/mirrorlist "$inputs/arch-mirrorlist"
pacman-key --init
pacman-key --populate archlinux
cp "$bundle/omarchy/x86_64/omarchy-keyring-20251027-1-any.pkg.tar.zst" "$inputs/omarchy-keyring.pkg.tar.zst"
printf '%s  %s\n' f4bc258879f493462736f9a072c4f4c7f753eb559f6b346f5bdf8ed816f75f66 \
    "$inputs/omarchy-keyring.pkg.tar.zst" | sha256sum -c -
pacman -U --noconfirm "$inputs/omarchy-keyring.pkg.tar.zst"
pacman-key --populate omarchy
# Use the desktop producer's pacman options and its dated Arch package set so
# the base and desktop share their distribution ABI and signature policy.
repo=https://raw.githubusercontent.com/omacom/omarchy/237f1b4071a0bff2296c6d740ba2b9fd695ebfa0/default/pacman
verified_download "$repo/pacman-stable.conf" \
    f8a4933a4d58b0935cc660da888cda2b4246c3c0926464d3e2e0e432c9354d70 "$inputs/pacman-stable.conf"
# The base takes no package from Omarchy's own repository, so only core, extra
# and multilib remain: the bundle's copies of the dated Arch Linux Archive.
# pacman still verifies every package signature against the keyrings.
sed '/^\[omarchy\]$/,/^$/d' "$inputs/pacman-stable.conf" > /etc/pacman.conf
printf 'Server = file://%s/$repo/os/$arch\n' "$bundle" > /etc/pacman.d/mirrorlist
test "$(pacman-conf --repo-list | paste -sd ' ')" = 'core extra multilib'
for repository in core extra multilib; do
    test "$(pacman-conf --repo="$repository" Server)" = "file://$bundle/$repository/os/x86_64"
done
pacman -Syu --disable-sandbox --needed --noconfirm python gcc make patch perl linux-api-headers
# The Arch container omits files from the installed development headers.
pacman -S --disable-sandbox --noconfirm glibc
bash /tmp/build-arch-openssl-cli.sh /arch-openssl-cli /tmp/openssl-remove-benchmarks.patch /tmp/verify-openssl-cli-hardening.sh
# Preserve the base image's documented omission profile when repopulating its root.
{
    cat /etc/pacman.conf
    printf '\n[options]\n'
    grep -E '^NoExtract[[:space:]]*=' "$inputs/arch-pacman.conf"
} > "$inputs/assembly.conf"
mkdir "$payload"
mkdir -p "$payload"/var/lib/pacman "$payload"/etc
pacman --root "$payload" --config "$inputs/assembly.conf" --gpgdir /etc/pacman.d/gnupg --cachedir /var/cache/pacman/pkg \
    -Sy --disable-sandbox --needed --noconfirm base
install -m 0644 "$inputs/arch-pacman.conf" "$payload/etc/pacman.conf"
install -m 0644 "$inputs/arch-mirrorlist" "$payload/etc/pacman.d/mirrorlist"
# Initialize the target filesystem without relying on mounts in the package chroot.
systemd-tmpfiles --root="$payload" -E --create
journalctl --root="$payload" --update-catalog
sh /tmp/install-arch-openssl-cli.sh /arch-openssl-cli "$payload"
python -B /tmp/normalize-omarchy.py "$payload"
python -B /tmp/normalize-machine-id.py "$payload"
python -B /tmp/assert-no-private-keyrings.py "$payload"

# Export only the fresh package root. Close and discard the build trust state
# before committing this build step, including when BuildKit retains its cache.
gpgconf --homedir /etc/pacman.d/gnupg --kill all
rm -rf /etc/pacman.d/gnupg /var/cache/pacman/pkg /var/lib/pacman/sync
echo "Assembled the signed Omarchy base with its staged package omissions"
