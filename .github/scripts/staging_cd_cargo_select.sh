#!/bin/sh
set -eu
set -o pipefail
export LC_ALL=C
registry=$1 target=$2 output=$3 byte_limit=$4 member_limit=$5
mkdir "$output"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' 0

if stat -c %s "$registry" >/dev/null 2>&1; then
    stat_flag=-c stat_format=%s
else
    stat_flag=-f stat_format=%z
fi

measure() {
    bytes=$(find "$1" -type f -exec stat "$stat_flag" "$stat_format" {} + |
        awk -v cap="$byte_limit" '{n += $1; if (n > cap) n = cap + 1} END {printf "%.0f\n", n}')
    members=$(find "$1" -print0 | tr -cd '\000' | wc -c)
}

for root in "$registry" "$target"; do
    test -d "$root" && test ! -L "$root"
    test "$(find "$root" ! -type f ! -type d -print0 | wc -c)" -eq 0
done
measure "$registry"
registry_bytes=$bytes registry_members=$members
measure "$target"
if test "$((registry_bytes + bytes))" -le "$byte_limit" &&
    test "$((registry_members + members))" -le "$member_limit"; then
    tar -C "$registry" -cf - . | gzip -1 > "$output/registry.tar.gz"
    tar -C "$target" -cf - . | gzip -1 > "$output/target.tar.gz"
    exit 0
fi

# A subset is supported only for the incremental-only layout. Workspace outputs
# and their source receipts require a separate complete-unit contract.
used_bytes=$registry_bytes used_members=$registry_members
: > "$scratch/selected"
for entry in "$target"/* "$target"/.[!.]* "$target"/..?*; do
    test -e "$entry" || continue
    case "${entry##*/}" in
        release) test -d "$entry" ;;
        .rustc_info.json|CACHEDIR.TAG)
            test -f "$entry"
            measure "$entry"
            used_bytes=$((used_bytes + bytes))
            used_members=$((used_members + members))
            printf './%s\n' "${entry##*/}" >> "$scratch/selected"
            ;;
        *) exit 1 ;;
    esac
done
test -d "$target/release/incremental"
for entry in "$target/release"/* "$target/release"/.[!.]* "$target/release"/..?*; do
    test -e "$entry" || continue
    test "${entry##*/}" = incremental
done
: > "$scratch/units"
for unit in "$target/release/incremental"/* "$target/release/incremental"/.[!.]* "$target/release/incremental"/..?*; do
    test -e "$unit" || continue
    test -d "$unit"
    name=${unit##*/}
    case "$name" in ''|*[!a-zA-Z0-9_-]*) exit 1 ;; esac
    completed=0 incomplete=0
    for state in "$unit"/s-*; do
        test -d "$state" || continue
        case "$state" in *-working) incomplete=1; continue ;; esac
        if test -f "$state/work-products.bin"; then
            completed=$((completed + 1))
        else
            incomplete=1
        fi
    done
    test "$completed" -gt 0 && test "$incomplete" -eq 0 || continue
    measure "$unit"
    printf '%s %s %s\n' "$bytes" "$members" "$name" >> "$scratch/units"
done
sort -k1,1nr -k3,3 "$scratch/units" > "$scratch/ordered"
kept=0
while read -r bytes members name; do
    if test "$((used_bytes + bytes))" -le "$byte_limit" &&
        test "$((used_members + members))" -le "$member_limit"; then
        printf './release/incremental/%s\n' "$name" >> "$scratch/selected"
        used_bytes=$((used_bytes + bytes))
        used_members=$((used_members + members))
        kept=$((kept + 1))
    fi
done < "$scratch/ordered"
if test "$kept" -eq 0; then
    touch "$output/oversize"
    exit 0
fi
tar -C "$registry" -cf - . | gzip -1 > "$output/registry.tar.gz"
tar -C "$target" -cf - -T "$scratch/selected" | gzip -1 > "$output/target.tar.gz"
