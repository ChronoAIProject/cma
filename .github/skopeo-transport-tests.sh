#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
export_script="$script_dir/export-release-image.sh"
import_script="$script_dir/import-release-images.sh"

skopeo_image='quay.io/skopeo/stable@sha256:4a16d57b37617a04b3d643079a477a2848efe892dffcdf0ce56df4262b65f810'
registry_image='registry:2.8.3@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373'
buildkit_image='moby/buildkit:v0.26.2@sha256:de10faf919fc71ba4eb1dd7bd6449566d012b0c9436b1c61bfee21d621b009aa'

version=v1.2.3
full_revision=abcdef0123456789abcdef0123456789abcdef01
revision=${full_revision:0:7}
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-skopeo-transport.XXXXXX")
work_dir=$(CDPATH='' cd -- "$work_dir" && pwd -P)
suffix=$(printf '%s' "${work_dir##*.}" | tr '[:upper:]' '[:lower:]')
network_name="cma-skopeo-$suffix"
registry_name="cma-registry-$suffix"
builder_name="cma-builder-$suffix"
builder_container="buildx_buildkit_${builder_name}0"
skopeo_container="cma-skopeo-run-$suffix"
registry_host="${registry_name}:5000"
docker_config="$work_dir/docker-config"
buildkit_config="$work_dir/buildkitd.toml"
skopeo_wrapper="$work_dir/run-skopeo"
skopeo_log="$work_dir/skopeo.log"
pulled_images=()
builder_volumes=()
registry_volumes=()
tests=0

mkdir -p "$docker_config"
: > "$skopeo_log"
export CMA_TRANSPORT_NETWORK="$network_name"
export CMA_TRANSPORT_REGISTRY="$registry_host"
export CMA_TRANSPORT_WORK_DIR="$work_dir"
export CMA_TRANSPORT_SKOPEO_IMAGE="$skopeo_image"
export CMA_TRANSPORT_SKOPEO_CONTAINER="$skopeo_container"
export CMA_TRANSPORT_SKOPEO_LOG="$skopeo_log"

fail() {
  printf 'not ok - %s\n' "$*" >&2
  exit 1
}

pass() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "$tests" "$1"
}

assert_contains() {
  grep -F -- "$2" "$1" >/dev/null \
    || fail "$1 does not contain: $2"
}

sha256_file() {
  local path sum
  path=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$path")
  else
    sum=$(shasum -a 256 "$path")
  fi
  printf 'sha256:%s\n' "${sum%% *}"
}

copy_count() {
  awk -F '\t' '$1 == "copy" { count++ } END { print count + 0 }' \
    "$skopeo_log"
}

clear_skopeo_log() {
  : > "$skopeo_log"
}

pull_pinned_image() {
  local image attempt delay
  image=$1
  for attempt in 1 2 3 4; do
    if docker pull "$image" >/dev/null; then
      return 0
    fi
    if [ "$attempt" -lt 4 ]; then
      delay=$((attempt * 10))
      printf 'Pinned image pull failed (%s/4); retrying in %ss: %s\n' \
        "$attempt" "$delay" "$image" >&2
      sleep "$delay"
    fi
  done
  fail "could not pull pinned image $image after 4 attempts"
}

cleanup() {
  local status cleanup_failed image resource volume
  status=$1
  cleanup_failed=0
  trap - EXIT HUP INT TERM
  set +e
  set +u

  docker container rm --force "$skopeo_container" >/dev/null 2>&1
  docker buildx rm --force "$builder_name" >/dev/null 2>&1
  docker container rm --force "$builder_container" >/dev/null 2>&1
  docker container rm --force --volumes "$registry_name" >/dev/null 2>&1
  for volume in "${registry_volumes[@]}"; do
    docker volume rm "$volume" >/dev/null 2>&1
  done
  for volume in "${builder_volumes[@]}"; do
    docker volume rm "$volume" >/dev/null 2>&1
  done
  for volume in "${registry_volumes[@]}"; do
    if docker volume inspect "$volume" >/dev/null 2>&1; then
      printf 'cleanup error: registry volume remains: %s\n' "$volume" >&2
      cleanup_failed=1
    fi
  done
  docker network rm "$network_name" >/dev/null 2>&1
  for image in "${pulled_images[@]}"; do
    for _ in {1..10}; do
      if ! docker image inspect "$image" >/dev/null 2>&1; then
        break
      fi
      docker image rm "$image" >/dev/null 2>&1 && break
      sleep 1
    done
  done

  for resource in "$skopeo_container" "$builder_container" "$registry_name"; do
    if docker container inspect "$resource" >/dev/null 2>&1; then
      printf 'cleanup error: Docker container remains: %s\n' "$resource" >&2
      cleanup_failed=1
    fi
  done
  if docker network inspect "$network_name" >/dev/null 2>&1; then
    printf 'cleanup error: Docker network remains: %s\n' "$network_name" >&2
    cleanup_failed=1
  fi
  for volume in "${builder_volumes[@]}"; do
    if docker volume inspect "$volume" >/dev/null 2>&1; then
      printf 'cleanup error: BuildKit volume remains: %s\n' "$volume" >&2
      cleanup_failed=1
    fi
  done
  for image in "${pulled_images[@]}"; do
    if docker image inspect "$image" >/dev/null 2>&1; then
      printf 'cleanup error: test-pulled image remains: %s\n' "$image" >&2
      cleanup_failed=1
    fi
  done

  rm -rf -- "$work_dir"
  if [ -e "$work_dir" ]; then
    printf 'cleanup error: integration work directory remains: %s\n' \
      "$work_dir" >&2
    cleanup_failed=1
  fi
  if [ "$cleanup_failed" -ne 0 ] && [ "$status" -eq 0 ]; then
    status=1
  fi
  exit "$status"
}
trap 'cleanup $?' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

command -v docker >/dev/null 2>&1 || fail 'Docker is required'
command -v jq >/dev/null 2>&1 || fail 'jq is required'
docker version >/dev/null || fail 'the Docker daemon is unavailable'
docker buildx version >/dev/null || fail 'Docker Buildx is required'

# Docker Desktop installs Buildx below the ordinary user config directory.
# Expose only that executable when switching to an otherwise empty config so
# host registry credentials and credential helpers remain out of this test.
buildx_plugin=$(docker info --format '{{json .ClientInfo.Plugins}}' \
  | jq -r '.[] | select(.Name == "buildx") | .Path' | head -n 1)
if [ -n "$buildx_plugin" ] && [ -x "$buildx_plugin" ]; then
  mkdir -p "$docker_config/cli-plugins"
  ln -s "$buildx_plugin" "$docker_config/cli-plugins/docker-buildx"
fi
export DOCKER_CONFIG="$docker_config"
docker buildx version >/dev/null \
  || fail 'Docker Buildx is unavailable from the isolated Docker config'

for image in "$skopeo_image" "$registry_image" "$buildkit_image"; do
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    pulled_images+=("$image")
  fi
  pull_pinned_image "$image"
done

docker network create "$network_name" >/dev/null \
  || fail 'could not create the isolated Docker network'
docker run --detach --name "$registry_name" --network "$network_name" \
  "$registry_image" >/dev/null \
  || fail 'could not start the pinned test registry'
while IFS= read -r volume; do
  [ -z "$volume" ] || registry_volumes+=("$volume")
done < <(docker container inspect --format \
  '{{range .Mounts}}{{if eq .Type "volume"}}{{println .Name}}{{end}}{{end}}' \
  "$registry_name")

registry_ready=false
for _ in {1..30}; do
  if docker logs "$registry_name" 2>&1 | grep -F 'listening on' >/dev/null; then
    registry_ready=true
    break
  fi
  if [ "$(docker inspect --format '{{.State.Running}}' "$registry_name")" != true ]; then
    break
  fi
  sleep 1
done
[ "$registry_ready" = true ] || fail 'the pinned test registry did not start'

cat > "$buildkit_config" <<EOF
[registry."$registry_host"]
  http = true
  insecure = true
EOF

docker buildx create \
  --name "$builder_name" \
  --driver docker-container \
  --driver-opt "image=$buildkit_image" \
  --driver-opt "network=$network_name" \
  --buildkitd-config "$buildkit_config" \
  --use >/dev/null \
  || fail 'could not create the pinned ephemeral BuildKit builder'
docker buildx inspect --builder "$builder_name" --bootstrap >/dev/null \
  || fail 'the pinned ephemeral BuildKit builder did not start'
while IFS= read -r volume; do
  [ -z "$volume" ] || builder_volumes+=("$volume")
done < <(docker container inspect --format \
  '{{range .Mounts}}{{if eq .Type "volume"}}{{println .Name}}{{end}}{{end}}' \
  "$builder_container")

cat > "$skopeo_wrapper" <<'SKOPEO'
#!/usr/bin/env bash
set -euo pipefail
: "${CMA_TRANSPORT_NETWORK:?}"
: "${CMA_TRANSPORT_REGISTRY:?}"
: "${CMA_TRANSPORT_WORK_DIR:?}"
: "${CMA_TRANSPORT_SKOPEO_IMAGE:?}"
: "${CMA_TRANSPORT_SKOPEO_CONTAINER:?}"
: "${CMA_TRANSPORT_SKOPEO_LOG:?}"
set +u

caller_workdir=$(pwd -P)
case "$caller_workdir" in
  "$CMA_TRANSPORT_WORK_DIR" | "$CMA_TRANSPORT_WORK_DIR"/*) ;;
  *)
    printf 'wrapper caller is outside the mounted test directory: %s\n' \
      "$caller_workdir" >&2
    exit 2
    ;;
esac

command_name=$1
shift
arguments=()
for argument in "$@"; do
  case "$argument" in
    docker://ghcr.io/chronoaiproject/*)
      argument="docker://${CMA_TRANSPORT_REGISTRY}/source/${argument#docker://ghcr.io/chronoaiproject/}"
      ;;
    docker://registry.example.com/*)
      argument="docker://${CMA_TRANSPORT_REGISTRY}/destination/${argument#docker://registry.example.com/}"
      ;;
  esac
  arguments+=("$argument")
done

{
  printf '%s' "$command_name"
  printf '\t%s' "${arguments[@]}"
  printf '\n'
} >> "$CMA_TRANSPORT_SKOPEO_LOG"

case "$command_name" in
  inspect | list-tags)
    arguments=("$command_name" --tls-verify=false "${arguments[@]}")
    ;;
  copy)
    arguments=("$command_name" --src-tls-verify=false \
      --dest-tls-verify=false "${arguments[@]}")
    ;;
  *)
    arguments=("$command_name" "${arguments[@]}")
    ;;
esac

exec docker run --rm --name "$CMA_TRANSPORT_SKOPEO_CONTAINER" \
  --network "$CMA_TRANSPORT_NETWORK" \
  --user "$(id -u):$(id -g)" \
  --env HOME=/tmp \
  --volume "$CMA_TRANSPORT_WORK_DIR:$CMA_TRANSPORT_WORK_DIR:rw" \
  --workdir "$caller_workdir" \
  "$CMA_TRANSPORT_SKOPEO_IMAGE" "${arguments[@]}"
SKOPEO
chmod 0700 "$skopeo_wrapper"

run_skopeo() (
  cd "$work_dir"
  "$skopeo_wrapper" "$@"
)

skopeo_version=$(run_skopeo --version)
printf '%s\n' "$skopeo_version" \
  | grep -Eq '^skopeo version 1\.22\.2( commit: [0-9a-f]{7,40})?$' \
  || fail "unexpected pinned Skopeo version: $skopeo_version"
pass 'pinned Skopeo, registry, and isolated BuildKit are available'

build_fixture() {
  local component platforms oci_media context repository
  component=$1
  platforms=$2
  oci_media=$3
  context="$work_dir/context-$component-$oci_media"
  repository="$registry_host/source/cma-$component"
  mkdir -p "$context"
  printf 'fixture payload for %s\n' "$component" > "$context/payload"
  cat > "$context/Dockerfile" <<'DOCKERFILE'
FROM scratch
ARG VERSION
ARG REVISION
LABEL org.opencontainers.image.version=$VERSION
LABEL org.opencontainers.image.revision=$REVISION
COPY payload /component-payload
DOCKERFILE

  if [ "$oci_media" = true ]; then
    docker buildx build \
      --builder "$builder_name" \
      --platform "$platforms" \
      --build-arg "VERSION=$version" \
      --build-arg "REVISION=$full_revision" \
      --provenance=false \
      --sbom=false \
      --output type=registry,oci-mediatypes=true \
      --tag "$repository:$version" \
      "$context" >/dev/null
  else
    docker buildx build \
      --builder "$builder_name" \
      --platform "$platforms" \
      --build-arg "VERSION=$version" \
      --build-arg "REVISION=$full_revision" \
      --provenance=false \
      --sbom=false \
      --output type=registry,oci-mediatypes=false \
      --tag "$repository:$version" \
      "$context" >/dev/null
  fi
}

create_source_alias() {
  local component repository raw digest
  component=$1
  repository="$registry_host/source/cma-$component"
  raw="$work_dir/$component-source.raw"
  run_skopeo inspect --raw "docker://${repository}:${version}" > "$raw"
  digest=$(sha256_file "$raw")
  run_skopeo copy --all --preserve-digests \
    "docker://${repository}@${digest}" \
    "docker://${repository}:${revision}" >/dev/null
  printf '%s\n' "$digest" > "$work_dir/$component.digest"
}

build_fixture web 'linux/amd64,linux/arm64' false
create_source_alias web
mkdir "$work_dir/rejected"
clear_skopeo_log
if SKOPEO_COMMAND="$skopeo_wrapper" \
  "$export_script" web "$version" "$full_revision" \
  ghcr.io/chronoaiproject/cma-web "$work_dir/rejected" \
  > "$work_dir/rejected.stdout" 2> "$work_dir/rejected.stderr"; then
  fail 'the exporter accepted a Docker manifest list'
fi
assert_contains "$work_dir/rejected.stderr" \
  'web source is not the exact OCI linux/amd64 plus linux/arm64 image index'
[ "$(copy_count)" -eq 0 ] \
  || fail 'the rejected Docker manifest list reached archive copy'
[ -z "$(find "$work_dir/rejected" -mindepth 1 -maxdepth 1 -print -quit)" ] \
  || fail 'the rejected Docker manifest list left archive output behind'
pass 'the real exporter rejects Docker media types before archive copy'

build_fixture api 'linux/amd64' true
build_fixture web 'linux/amd64,linux/arm64' true
build_fixture runtime 'linux/amd64' true
build_fixture cmaeg 'linux/amd64' true
build_fixture image-manager 'linux/amd64' true
for component in api web runtime cmaeg image-manager; do
  create_source_alias "$component"
done
pass 'release-shaped OCI fixtures and digest aliases were published locally'

transfer_dir="$work_dir/transfer"
mkdir "$transfer_dir"
for component in api web runtime cmaeg image-manager; do
  mkdir "$transfer_dir/$component"
  SKOPEO_COMMAND="$skopeo_wrapper" \
    "$export_script" "$component" "$version" "$full_revision" \
    "ghcr.io/chronoaiproject/cma-$component" \
    "$transfer_dir/$component" >/dev/null
done
pass 'the real exporter preserved all five source root digests in OCI archives'

verify_platform_config() {
  local component architecture manifest source_reference destination_reference prefix
  local expected_config config_hash raw_config source_config destination_config
  component=$1
  architecture=$2
  manifest=$3
  source_reference=$4
  destination_reference=$5
  prefix=$6
  expected_config=$(jq -er '.config.digest' "$manifest")
  source_config="$work_dir/${prefix}-source-$architecture.config"
  destination_config="$work_dir/${prefix}-destination-$architecture.config"
  run_skopeo inspect --config --override-os linux \
    --override-arch "$architecture" "$source_reference" > "$source_config"
  run_skopeo inspect --config --override-os linux \
    --override-arch "$architecture" "$destination_reference" \
    > "$destination_config"
  cmp -s "$source_config" "$destination_config" \
    || fail "$component linux/$architecture destination config bytes differ"
  # `skopeo inspect --config` renders JSON, so its bytes are not the bytes
  # addressed by the manifest descriptor. Read the literal pinned-registry
  # blob to prove the descriptor and payload match.
  config_hash=${expected_config#sha256:}
  raw_config="$work_dir/${prefix}-raw-$architecture.config"
  docker container exec "$registry_name" cat \
    "/var/lib/registry/docker/registry/v2/blobs/sha256/${config_hash:0:2}/${config_hash}/data" \
    > "$raw_config"
  [ "$(sha256_file "$raw_config")" = "$expected_config" ] \
    || fail "$component linux/$architecture destination config digest differs"
  jq -e --arg architecture "$architecture" --arg version "$version" \
    --arg revision "$full_revision" '
      .os == "linux" and .architecture == $architecture and
      .config.Labels["org.opencontainers.image.version"] == $version and
      .config.Labels["org.opencontainers.image.revision"] == $revision
    ' "$raw_config" >/dev/null \
    || fail "$component linux/$architecture destination config labels differ"
}

verify_destination() {
  local destination_prefix component metadata source_repository destination_repository
  local expected_digest tag source_raw destination_raw architecture child_digest
  local source_child destination_child prefix
  destination_prefix=$1
  for component in api web runtime cmaeg image-manager; do
    metadata="$transfer_dir/$component/cma-${component}-${version}-${revision}.json"
    source_repository="ghcr.io/chronoaiproject/cma-$component"
    destination_repository="${destination_prefix}/cma-$component"
    expected_digest=$(jq -er '.sourceDigest' "$metadata")
    for tag in "$version" "$revision"; do
      prefix=$(printf '%s-%s-%s' "${destination_prefix//\//-}" "$component" "$tag")
      source_raw="$work_dir/${prefix}-source.raw"
      destination_raw="$work_dir/${prefix}-destination.raw"
      run_skopeo inspect --raw \
        "docker://${source_repository}@${expected_digest}" > "$source_raw"
      run_skopeo inspect --raw \
        "docker://${destination_repository}:${tag}" > "$destination_raw"
      cmp -s "$source_raw" "$destination_raw" \
        || fail "$destination_repository:$tag root bytes differ from the source"
      [ "$(sha256_file "$destination_raw")" = "$expected_digest" ] \
        || fail "$destination_repository:$tag root digest differs from metadata"

      if [ "$component" = web ]; then
        jq -e '
          .mediaType == "application/vnd.oci.image.index.v1+json" and
          ([.manifests[].platform | (.os + "/" + .architecture)] | sort ==
           ["linux/amd64", "linux/arm64"])
        ' "$destination_raw" >/dev/null \
          || fail "$destination_repository:$tag has the wrong platform index"
        while IFS=$'\t' read -r architecture child_digest; do
          source_child="$work_dir/${prefix}-source-$architecture.manifest"
          destination_child="$work_dir/${prefix}-destination-$architecture.manifest"
          run_skopeo inspect --raw \
            "docker://${source_repository}@${child_digest}" > "$source_child"
          run_skopeo inspect --raw \
            "docker://${destination_repository}@${child_digest}" \
            > "$destination_child"
          cmp -s "$source_child" "$destination_child" \
            || fail "$destination_repository:$tag linux/$architecture child bytes differ"
          [ "$(sha256_file "$destination_child")" = "$child_digest" ] \
            || fail "$destination_repository:$tag linux/$architecture child digest differs"
          verify_platform_config "$component" "$architecture" "$destination_child" \
            "docker://${source_repository}@${expected_digest}" \
            "docker://${destination_repository}:${tag}" "$prefix"
        done < <(jq -r '.manifests[] | [.platform.architecture, .digest] | @tsv' \
          "$destination_raw")
      else
        jq -e '
          .mediaType == "application/vnd.oci.image.manifest.v1+json" and
          (has("manifests") | not)
        ' "$destination_raw" >/dev/null \
          || fail "$destination_repository:$tag is not a single OCI manifest"
        verify_platform_config "$component" amd64 "$destination_raw" \
          "docker://${source_repository}@${expected_digest}" \
          "docker://${destination_repository}:${tag}" "$prefix"
      fi
    done
  done
}

run_import() {
  local destination_prefix stdout stderr
  destination_prefix=$1
  stdout=$2
  stderr=$3
  SKOPEO_COMMAND="$skopeo_wrapper" \
    "$import_script" "$transfer_dir" "$version" "$full_revision" \
    "$destination_prefix" > "$stdout" 2> "$stderr"
}

absent_destination=registry.example.com/all-absent
clear_skopeo_log
run_import "$absent_destination" "$work_dir/absent.stdout" \
  "$work_dir/absent.stderr" || fail 'the ten-absent-tag import failed'
[ "$(copy_count)" -eq 10 ] || fail 'the ten-absent-tag import did not copy ten tags'
verify_destination "$absent_destination"
pass 'ten absent destination tags copy with exact roots, children, and configs'

clear_skopeo_log
run_import "$absent_destination" "$work_dir/equal.stdout" \
  "$work_dir/equal.stderr" || fail 'the all-equal retry failed'
[ "$(copy_count)" -eq 0 ] || fail 'the all-equal retry copied a tag'
pass 'an all-equal retry performs zero copies'

partial_destination=registry.example.com/partial
api_digest=$(<"$work_dir/api.digest")
run_skopeo copy --all --preserve-digests \
  "docker://${registry_host}/source/cma-api@${api_digest}" \
  "docker://${partial_destination}/cma-api:${version}" >/dev/null
clear_skopeo_log
run_import "$partial_destination" "$work_dir/partial.stdout" \
  "$work_dir/partial.stderr" || fail 'the one-preseeded-tag import failed'
[ "$(copy_count)" -eq 9 ] || fail 'the one-preseeded-tag import did not copy nine tags'
verify_destination "$partial_destination"
clear_skopeo_log
run_import "$partial_destination" "$work_dir/partial-retry.stdout" \
  "$work_dir/partial-retry.stderr" || fail 'the completed partial retry failed'
[ "$(copy_count)" -eq 0 ] || fail 'the completed partial retry copied a tag'
pass 'one exact preseed copies nine tags, then a retry copies zero'

mismatch_destination=registry.example.com/mismatch
runtime_digest=$(<"$work_dir/runtime.digest")
run_skopeo copy --all --preserve-digests \
  "docker://${registry_host}/source/cma-runtime@${runtime_digest}" \
  "docker://${mismatch_destination}/cma-api:${version}" >/dev/null
clear_skopeo_log
if run_import "$mismatch_destination" "$work_dir/mismatch.stdout" \
  "$work_dir/mismatch.stderr"; then
  fail 'a mismatched destination tag was accepted'
fi
[ "$(copy_count)" -eq 0 ] \
  || fail 'the mismatched destination was detected after a copy began'
assert_contains "$work_dir/mismatch.stderr" \
  'destination tag has a different digest'
pass 'one mismatched destination tag aborts before every copy'

printf '1..%d\n' "$tests"
