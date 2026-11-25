"""Bounded, read-only export of Rust service Cargo cache mounts."""


def export_script(max_expanded, max_compressed, max_members, *, incremental_only=False):
    # Measurements include the tar headers and padding, so the unpacked-byte
    # budget is conservative relative to the bundle validator's member sizes.
    limits = (
        f"max_expanded={int(max_expanded)}\n"
        f"max_compressed={int(max_compressed)}\n"
        f"max_members={int(max_members)}\n"
        f"target_entry={'./release/incremental' if incremental_only else '.'}\n"
    )
    return limits + r'''set -euo pipefail
export LC_ALL=C
registry=$1
target=$2
output=$3
mkdir -p "$output"
registry_archive="$output/registry.tar.gz"
target_archive="$output/target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkfifo "$tmp/raw"

archive_stream() {
    source=$1
    archive=$2
    shift 2
    # Count the actual tar stream without decompressing the completed archive
    # a second time. Keep the independent member-list and seal checks.
    wc -c < "$tmp/raw" > "$tmp/raw-count" &
    counter=$!
    tar -C "$source" -cf - "$@" | tee "$tmp/raw" | gzip -1 > "$archive"
    wait "$counter"
    archive_compressed=$(wc -c < "$archive")
    archive_expanded=$(cat "$tmp/raw-count")
    archive_members=$(tar -tzf "$archive" | wc -l)
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
    archive_stream "$registry" "$registry_archive" $skip_src .
    registry_compressed=$archive_compressed
    registry_expanded=$archive_expanded
    registry_members=$archive_members
}

[ -d "$target/$target_entry" ] || refuse empty
total=$(du -sk "$registry" "$target/$target_entry" | awk '{n += $1} END {print n}')
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

attempts=0
if [ "$total" -le "$bound_kib" ]; then
    attempts=1
    archive_stream "$target" "$target_archive" "$target_entry"
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

# Work within the existing artifact budgets. The normal Cargo build handles
# any omitted group, and rustc still decides whether compiler state is usable.
visited_names=""
remaining_expanded=$((max_expanded - registry_expanded))
remaining_compressed=$((max_compressed - registry_compressed))
remaining_members=$((max_members - registry_members))
: > "$tmp/candidates"
mkdir "$tmp/preferred"
for group in "$target"/release/incremental/cma-* \
    "$target"/release/incremental/cmaeg-* \
    "$target"/release/incremental/cmaeg_*-* \
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
        work_products="$state/work-products.bin"
        if [ ! -f "$work_products" ]; then
            completed=-1
            break
        fi
        completed=$((completed + 1))
    done
    [ "$completed" -eq 1 ] || continue
    kib=$(du -sk "$group" | cut -f1)
    members=$(find "$group" -print | wc -l)
    # Estimate the tar allocation, including a header and possible padding for
    # each member. Exact archive checks remain authoritative for sparse files.
    expanded=$((kib * 1024 + members * 1024 + 10240))
    printf '%s %s %s\n' "release/incremental/$name" "$expanded" "$members" >> "$tmp/candidates"
    # A dependency graph change gives the same crate a new incremental group.
    # Prefer its latest completed work products by mtime, for ranking only.
    preferred="$tmp/preferred/${name%-*}"
    if [ ! -f "$preferred" ] || [ "$work_products" -nt "$target/$(cat "$preferred")" ]; then
        printf '%s\n' "${work_products#"$target/"}" > "$preferred"
    fi
done

# Give each distinct crate a chance before spending the budget on duplicate
# generations. Preserve the component priorities and allow older units if
# there is room left (some packages have more than one valid compiler unit).
: > "$tmp/primary"
: > "$tmp/secondary"
while read -r relative expanded members; do
    name=${relative##*/}
    preferred=$(cat "$tmp/preferred/${name%-*}")
    case "$preferred" in
        "$relative"/*) candidates="$tmp/primary" ;;
        *) candidates="$tmp/secondary" ;;
    esac
    printf '%s %s %s\n' "$relative" "$expanded" "$members" >> "$candidates"
done < "$tmp/candidates"
cat "$tmp/primary" "$tmp/secondary" > "$tmp/candidates"

# Estimate compressed contribution using the previous observed roughly 2:1
# ratio. Every accepted artifact must still pass the exact compressed bound.
set --
expanded_left=$remaining_expanded
compressed_left=$remaining_compressed
members_left=$remaining_members
while read -r relative expanded members; do
    compressed=$(((expanded + 1) / 2))
    if [ "$expanded" -le "$expanded_left" ] &&
       [ "$compressed" -le "$compressed_left" ] &&
       [ "$members" -le "$members_left" ]; then
        set -- "$@" "$relative"
        expanded_left=$((expanded_left - expanded))
        compressed_left=$((compressed_left - compressed))
        members_left=$((members_left - members))
    fi
done < "$tmp/candidates"

# A full target archive, including any initial whole-target attempt, counts
# toward the same rewrite budget. When actual sizes exceed the estimate,
# preserve a deterministic prefix of the selected complete crate groups.
while [ "$attempts" -lt 3 ] && [ "$#" -gt 0 ]; do
    attempts=$((attempts + 1))
    archive_stream "$target" "$target_archive" "$@"
    if fits "$((registry_expanded + archive_expanded))" \
        "$((registry_compressed + archive_compressed))" \
        "$((registry_members + archive_members))"; then
        printf '%s\n' "$#" > "$output/compacted"
        exit 0
    fi
    keep=$(($# - 1))
    if [ "$archive_compressed" -gt "$remaining_compressed" ]; then
        limit=$(($# * remaining_compressed / archive_compressed))
        [ "$limit" -ge "$keep" ] || keep=$limit
    fi
    if [ "$archive_expanded" -gt "$remaining_expanded" ]; then
        limit=$(($# * remaining_expanded / archive_expanded))
        [ "$limit" -ge "$keep" ] || keep=$limit
    fi
    if [ "$archive_members" -gt "$remaining_members" ]; then
        limit=$(($# * remaining_members / archive_members))
        [ "$limit" -ge "$keep" ] || keep=$limit
    fi
    # Rotate the retained prefix to the end and remove the suffix.
    seen=0
    count=$#
    while [ "$seen" -lt "$count" ]; do
        next=$1
        shift
        [ "$seen" -ge "$keep" ] || set -- "$@" "$next"
        seen=$((seen + 1))
    done
done
[ "$attempts" -gt 0 ] || refuse empty
[ "$#" -gt 0 ] || refuse empty
refuse_budget "$((registry_expanded + archive_expanded))" \
    "$((registry_compressed + archive_compressed))"
'''
