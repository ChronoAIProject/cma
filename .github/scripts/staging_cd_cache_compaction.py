"""Bounded, read-only export of AM Cargo cache mounts."""


def export_script(max_expanded, max_compressed, max_members):
    # Measurements include the tar headers and padding, so the unpacked-byte
    # budget is conservative relative to the bundle validator's member sizes.
    limits = (
        f"max_expanded={int(max_expanded)}\n"
        f"max_compressed={int(max_compressed)}\n"
        f"max_members={int(max_members)}\n"
    )
    return limits + r'''set -euo pipefail
export LC_ALL=C
registry=$1
target=$2
output=$3
mkdir -p "$output"
registry_archive="$output/registry.tar.gz"
target_archive="$output/target.tar.gz"

measure() {
    archive_compressed=$(wc -c < "$1")
    archive_expanded=$(gzip -dc "$1" | wc -c)
    archive_members=$(tar -tzf "$1" | wc -l)
}

fits() {
    [ "$1" -le "$max_expanded" ] &&
    [ "$2" -le "$max_compressed" ] &&
    [ "$3" -le "$max_members" ]
}

refuse() {
    rm -f "$registry_archive" "$target_archive"
    touch "$output/$1"
    exit 0
}

refuse_budget() {
    if [ "$2" -gt "$max_compressed" ]; then
        refuse oversize-compressed
    fi
    refuse oversize
}

archive_registry() {
    tar -C "$registry" -cf - $skip_src . | gzip -1 > "$registry_archive"
    measure "$registry_archive"
    registry_compressed=$archive_compressed
    registry_expanded=$archive_expanded
    registry_members=$archive_members
}

total=$(du -sk "$registry" "$target" | awk '{n += $1} END {print n}')
bound_kib=$((max_expanded / 1024))
skip_src=
if [ "$total" -gt "$((bound_kib * 3 / 4))" ] && [ -d "$registry/src" ]; then
    src=$(du -sk "$registry/src" | cut -f1)
    total=$((total - src))
    skip_src="--exclude=./src"
fi
archive_registry
if ! fits "$registry_expanded" "$registry_compressed" "$registry_members"; then
    if [ -z "$skip_src" ] && [ -d "$registry/src" ]; then
        skip_src="--exclude=./src"
        archive_registry
    fi
    fits "$registry_expanded" "$registry_compressed" "$registry_members" ||
        refuse_budget "$registry_expanded" "$registry_compressed"
fi

if [ "$total" -le "$bound_kib" ]; then
    tar -C "$target" -cf - . | gzip -1 > "$target_archive"
    measure "$target_archive"
    if fits "$((registry_expanded + archive_expanded))" \
        "$((registry_compressed + archive_compressed))" \
        "$((registry_members + archive_members))"; then
        exit 0
    fi
    rm -f "$target_archive"
fi

# Remove reconstructible source trees if a compressed-only miss still has them.
if [ -z "$skip_src" ] && [ -d "$registry/src" ]; then
    skip_src="--exclude=./src"
    archive_registry
fi

# Work within the existing artifact budgets. Select complete crate groups in
# stable priority order. The normal Cargo build handles any omitted group.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
set --
visited_names=""
remaining_expanded=$((max_expanded - registry_expanded))
remaining_compressed=$((max_compressed - registry_compressed))
remaining_members=$((max_members - registry_members))
for group in "$target"/release/incremental/cma-* \
    "$target"/release/incremental/layer2_core-* \
    "$target"/release/incremental/layer1_public-* \
    "$target"/release/incremental/layer3_adapter-* \
    "$target"/release/incremental/*; do
    [ -d "$group" ] || continue
    name=${group##*/}
    case "$name" in ""|*[!a-zA-Z0-9_-]*) continue ;; esac
    case " $visited_names " in *" $name "*) continue ;; esac
    # Also avoid retrying a skipped group at the end of the priority list.
    visited_names="$visited_names $name"

    # The image trims each crate to its latest completed session. Any group
    # with an unfinished or ambiguous session remains a cold rebuild.
    completed=0
    for state in "$group"/s-*; do
        [ -d "$state" ] || continue
        case "$state" in *-working) completed=-1; break ;; esac
        if [ ! -f "$state/work-products.bin" ]; then
            completed=-1
            break
        fi
        completed=$((completed + 1))
    done
    [ "$completed" -eq 1 ] || continue
    kib=$(du -sk "$group" | cut -f1)
    [ "$((kib * 1024))" -le "$remaining_expanded" ] || continue

    relative="release/incremental/$name"
    # Each provisional archive is independent. A hard link whose other
    # name lies outside this group is stored with its contents.
    tar -C "$target" -cf - "$relative" | gzip -1 > "$tmp/group.tar.gz"
    measure "$tmp/group.tar.gz"
    if [ "$archive_expanded" -le "$remaining_expanded" ] &&
       [ "$archive_compressed" -le "$remaining_compressed" ] &&
       [ "$archive_members" -le "$remaining_members" ]; then
        set -- "$@" "$relative"
        remaining_expanded=$((remaining_expanded - archive_expanded))
        remaining_compressed=$((remaining_compressed - archive_compressed))
        remaining_members=$((remaining_members - archive_members))
    fi
    rm -f "$tmp/group.tar.gz"
done

[ "$#" -gt 0 ] || refuse empty
# Produce one actual tar containing only the chosen complete groups. Tar
# resolves hard links among names present in this export.
tar -C "$target" -cf - "$@" | gzip -1 > "$target_archive"
measure "$target_archive"
fits "$((registry_expanded + archive_expanded))" \
    "$((registry_compressed + archive_compressed))" \
    "$((registry_members + archive_members))" ||
    refuse_budget "$((registry_expanded + archive_expanded))" \
        "$((registry_compressed + archive_compressed))"
printf '%s\n' "$#" > "$output/compacted"
'''
