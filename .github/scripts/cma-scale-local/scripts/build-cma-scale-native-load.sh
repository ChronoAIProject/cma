#!/usr/bin/env bash
# Native build and kind-load CMA API plus Event Gateway for cma-scale.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build-cma-scale-native-load.sh <git-ref> [options]
Build both cma and cmaeg from an existing ref, then load and roll out cma-scale.
Options:
  --repo PATH          source checkout
  --cluster-root PATH  manager-02 scale directory
  --platform VALUE     linux/arm64 (default) or linux/amd64
  --builder NAME       buildx builder (default desktop-linux)
  --cache-root PATH    persistent cache root (default ./cma-scale-native-cache)
  --os-refresh VALUE   OS_SECURITY_REFRESH build arg (default local)
  --build-only         build/check images without loading or changing any cluster
  --image-prefix NAME  local image prefix (default cma-scale)
  -h, --help           show help
EOF
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo=${CMA_SCALE_REPO:-"$HOME/code/work/tech-lead.staff/local-env-manager-02/source"}
cluster_root=${CMA_SCALE_CLUSTER_ROOT:-"$HOME/code/work/tech-lead.staff/local-env-manager-02"}
platform=${PLATFORM:-linux/arm64}
builder=${BUILDER:-desktop-linux}
cache_root=${CACHE_ROOT:-"$script_dir/cma-scale-native-cache"}
os_refresh=${OS_SECURITY_REFRESH:-local}
build_slot=${BUILD_SLOT_WRAPPER:-"$HOME/code/work/.tl-env/build-slot"}
ref_arg=""
build_only=false
image_prefix=cma-scale

while (($#)); do
  case "$1" in
    --repo) (($# >= 2)) || { echo "--repo requires a path" >&2; exit 64; }; repo=$2; shift 2 ;;
    --cluster-root) (($# >= 2)) || { echo "--cluster-root requires a path" >&2; exit 64; }; cluster_root=$2; shift 2 ;;
    --platform) (($# >= 2)) || { echo "--platform requires a value" >&2; exit 64; }; platform=$2; shift 2 ;;
    --builder) (($# >= 2)) || { echo "--builder requires a name" >&2; exit 64; }; builder=$2; shift 2 ;;
    --cache-root) (($# >= 2)) || { echo "--cache-root requires a path" >&2; exit 64; }; cache_root=$2; shift 2 ;;
    --os-refresh) (($# >= 2)) || { echo "--os-refresh requires a value" >&2; exit 64; }; os_refresh=$2; shift 2 ;;
    --build-only) build_only=true; shift ;;
    --image-prefix) (($# >= 2)) || exit 64; image_prefix=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage; exit 64 ;;
    *) [[ -z "$ref_arg" ]] || { echo "only one ref may be supplied" >&2; usage; exit 64; }; ref_arg=$1; shift ;;
  esac
done

[[ -n "$ref_arg" ]] || { echo "a git ref or SHA is required" >&2; usage; exit 64; }
case "$platform" in
  linux/arm64) rust_target=aarch64-unknown-linux-musl; arch=arm64 ;;
  linux/amd64) rust_target=x86_64-unknown-linux-musl; arch=amd64 ;;
  *) echo "unsupported platform: $platform" >&2; exit 64 ;;
esac

require_command() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command is missing: $1" >&2; exit 127; }
}
require_command git
require_command gh
active_account=$(gh api user --jq .login)
[[ "$active_account" == chronoai-shining ]] || { echo "BLOCKED: gh account is $active_account, not chronoai-shining" >&2; exit 1; }
require_command docker
require_command cargo
require_command rustc
require_command python3
require_command file
require_command shasum
require_command awk
[[ -e "$repo/.git" ]] || { echo "not a Git checkout: $repo" >&2; exit 1; }
[[ -x "$build_slot" ]] || { echo "shared build-slot wrapper is required: $build_slot" >&2; exit 1; }
if command -v cargo-zigbuild >/dev/null 2>&1; then
  cargo zigbuild --help >/dev/null
elif ! cargo zigbuild --help >/dev/null 2>&1; then
  echo "cargo-zigbuild is required; put cargo-zigbuild on PATH" >&2
  exit 127
fi
require_command zig
target_libdir=$(rustc --print target-libdir --target "$rust_target" 2>/dev/null || true)
if [[ -z "$target_libdir" ]] || ! compgen -G "$target_libdir/libstd-*.rlib" >/dev/null; then
  echo "Rust target $rust_target is not installed for the selected toolchain" >&2
  echo "Install it in a project-local RUSTUP_HOME, or choose --platform linux/amd64 when x86_64-unknown-linux-musl is available" >&2
  exit 1
fi
[[ -f "$repo/infra/api/Dockerfile" ]] || { echo "API Dockerfile missing" >&2; exit 1; }
[[ -f "$repo/infra/cmaeg/Dockerfile" ]] || { echo "EG Dockerfile missing" >&2; exit 1; }
if [[ "$build_only" != true ]]; then
  [[ -f "$cluster_root/manifests/apps.json" ]] || { echo "apps manifest missing" >&2; exit 1; }
  [[ -x "$cluster_root/scripts/k" ]] || { echo "guarded k wrapper missing" >&2; exit 1; }
  [[ -x "$cluster_root/bin/kind" ]] || { echo "kind binary missing" >&2; exit 1; }
fi

commit=$(git -C "$repo" rev-parse --verify "$ref_arg^{commit}") || {
  echo "git ref is not present in $repo: $ref_arg" >&2
  exit 1
}
short=$(printf '%s' "$commit" | cut -c1-12)
git -C "$repo" cat-file -e "$commit^{commit}"
docker buildx inspect "$builder" >/dev/null

mkdir -p "$cache_root/cargo-target" "$cache_root/buildkit/api" "$cache_root/buildkit/eg"
cache_root=$(CDPATH='' cd -- "$cache_root" && pwd -P)
target_root="$cache_root/cargo-target"
api_cache="$cache_root/buildkit/api"
eg_cache="$cache_root/buildkit/eg"
context=$(mktemp -d "$script_dir/.cma-scale-context.XXXXXX")
manifest=$(mktemp "$script_dir/.cma-scale-apps.XXXXXX.json")
cleanup() {
  [[ -d "$context" ]] && rm -rf -- "$context"
  [[ -f "$manifest" ]] && rm -f -- "$manifest"
}
trap cleanup EXIT

echo "source_ref=$ref_arg"
echo "source_commit=$commit"
echo "platform=$platform"
echo "rust_target=$rust_target"
echo "cache_root=$cache_root"

git -C "$repo" archive --format=tar "$commit" | tar -xf - -C "$context"
mkdir -p "$context/native"

python3 - "$context/infra/api/Dockerfile" "$context/infra/api/Dockerfile.native" <<'PY'
from pathlib import Path
import sys
source, destination = map(Path, sys.argv[1:])
dockerfile = source.read_text()
build_start = dockerfile.find("FROM rust:1.96.1-alpine AS build")
runtime_start = dockerfile.find("FROM alpine:3.22 AS runtime", build_start + 1)
if build_start < 0 or runtime_start < 0:
    raise SystemExit("API Dockerfile stages changed")
replacement = "FROM scratch AS build\nCOPY native/cma /out/cma\n\n"
destination.write_text(dockerfile[:build_start] + replacement + dockerfile[runtime_start:])
PY

python3 - "$context/infra/cmaeg/Dockerfile" "$context/infra/cmaeg/Dockerfile.native" <<'PY'
from pathlib import Path
import re
import sys
source, destination = map(Path, sys.argv[1:])
dockerfile = source.read_text()
build_start = dockerfile.find("FROM rust:1.96.1-alpine AS build")
ffmpeg_start = dockerfile.find("FROM alpine:3.22 AS ffmpeg-build", build_start + 1)
runtime_start = dockerfile.find("FROM alpine:3.22 AS runtime", ffmpeg_start + 1)
if min(build_start, ffmpeg_start, runtime_start) < 0:
    raise SystemExit("EG Dockerfile stages changed")
replacement = "FROM scratch AS build\nCOPY native/cmaeg /out/cmaeg\n\n"
media_marker = (
    "FROM alpine:3.22 AS media-self-test\n"
    "RUN printf '%s\\n' 'media transformer disabled for local cma-scale image' > /tmp/media-self-test.txt\n\n"
)
runtime = dockerfile[runtime_start:]
runtime = re.sub(r"^COPY --from=ffmpeg-build .*\n", "", runtime, flags=re.MULTILINE)
runtime = runtime.replace(
    "FROM alpine:3.22 AS runtime\n",
    "FROM alpine:3.22 AS runtime\n"
    "LABEL dev.chrono.cmaeg.packaging=local-scale-no-transformer \\\n"
    "      dev.chrono.cmaeg.media-transformer=disabled\n",
    1,
)
destination.write_text(dockerfile[:build_start] + replacement + media_marker + runtime)
PY

compile_start=$(date +%s)
echo "compile_start=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
compile_command=(cargo zigbuild --release --locked --bin cma --bin cmaeg --target "$rust_target")
(
  cd "$context/api"
  CHRONO_BUILD="git-$short" CHRONO_BUILD_SHA="$commit" CARGO_TARGET_DIR="$target_root" "$build_slot" "${compile_command[@]}"
)
compile_end=$(date +%s)
echo "compile_seconds=$((compile_end - compile_start))"

native_api="$target_root/$rust_target/release/cma"
native_eg="$target_root/$rust_target/release/cmaeg"
for binary in "$native_api" "$native_eg"; do
  [[ -x "$binary" ]] || { echo "missing native binary: $binary" >&2; exit 1; }
  description=$(file "$binary")
  echo "binary=$description"
  case "$arch" in
    arm64) [[ "$description" == *"ARM aarch64"* ]] || { echo "wrong architecture for $binary" >&2; exit 1; } ;;
    amd64) [[ "$description" == *"x86-64"* ]] || { echo "wrong architecture for $binary" >&2; exit 1; } ;;
  esac
  [[ "$description" == *"statically linked"* ]] || { echo "non-static musl binary: $binary" >&2; exit 1; }
done
cp "$native_api" "$context/native/cma"
cp "$native_eg" "$context/native/cmaeg"

api_tag="$image_prefix-api:$arch-$short"
eg_tag="$image_prefix-eg:$arch-$short"
build_image() {
  local name=$1 tag=$2 dockerfile=$3 cache=$4
  local -a args=(
    docker buildx build --builder "$builder" --platform "$platform"
    --progress plain --file "$dockerfile" --tag "$tag"
    --provenance=false --load
    --build-arg "CHRONO_BUILD=git-$short"
    --build-arg "CHRONO_BUILD_SHA=$commit"
    --build-arg "OS_SECURITY_REFRESH=$os_refresh"
  )
  if [[ -f "$cache/index.json" ]]; then
    args+=(--cache-from "type=local,src=$cache")
  fi
  args+=(--cache-to "type=local,dest=$cache,mode=max" "$context")
  local started ended
  started=$(date +%s)
  echo "${name}_image=$tag"
  echo "${name}_image_start=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  "${args[@]}"
  ended=$(date +%s)
  echo "${name}_image_seconds=$((ended - started))"
}

build_image api "$api_tag" "$context/infra/api/Dockerfile.native" "$api_cache"
build_image eg "$eg_tag" "$context/infra/cmaeg/Dockerfile.native" "$eg_cache"

inspect_image() {
  local name=$1 tag=$2 binary=$3 expected_entrypoint=$4 expected_ports=$5
  python3 - "$name" "$tag" "$expected_entrypoint" "$expected_ports" <<'PY'
import json
import subprocess
import sys

name, tag, entrypoint, expected_ports = sys.argv[1:]
info = json.loads(subprocess.check_output(["docker", "image", "inspect", tag], text=True))[0]
config = info["Config"]
want_arch = "arm64" if tag.split(":", 1)[1].startswith("arm64-") else "amd64"
assert info["Architecture"] == want_arch, (name, info["Architecture"], want_arch)
assert info["Os"] == "linux", (name, info["Os"])
assert config["User"] == "10001:10001", (name, config["User"])
assert config["Entrypoint"] == [entrypoint], (name, config["Entrypoint"])
ports = sorted(config.get("ExposedPorts", {}).keys())
assert ports == expected_ports.split(","), (name, ports, expected_ports)
print(f"{name}_inspect arch={info['Architecture']} os={info['Os']} user={config['User']} entrypoint={config['Entrypoint']} ports={ports}")
PY
  local host_sha image_sha image_binary
  host_sha=$(shasum -a 256 "$binary" | awk '{print $1}')
  if [[ "$name" == api ]]; then image_binary=/usr/local/bin/cma; else image_binary=/usr/local/bin/cmaeg; fi
  image_sha=$(docker run --rm --platform "$platform" --entrypoint /bin/sh "$tag" -c "sha256sum $image_binary" | awk '{print $1}')
  [[ "$host_sha" == "$image_sha" ]] || { echo "$name binary checksum mismatch" >&2; exit 1; }
  docker run --rm --platform "$platform" --entrypoint /bin/sh "$tag" -c '
    set -eu
    test "$(id -u)" = 10001
    test "$(id -g)" = 10001
    test -x /usr/local/bin/codex-native
    test -x /usr/local/bin/gitleaks
    test -f /usr/local/share/gitleaks/source.txt
    test "$(/usr/local/bin/codex-native --version)" = "codex-cli 0.147.0"
    test "$(/usr/local/bin/gitleaks version)" = "8.30.1"
    test -f /etc/ssl/certs/ca-certificates.crt
  '
  echo "${name}_parity=entrypoint-user-ports-scanner-codex-ca-and-binary-sha-verified"
}

inspect_image api "$api_tag" "$native_api" /usr/local/bin/cma 8080/tcp
inspect_image eg "$eg_tag" "$native_eg" /usr/local/bin/cmaeg 8090/tcp,9091/tcp
docker run --rm --platform "$platform" --entrypoint /bin/sh "$eg_tag" -c \
  'grep -Fx "media transformer disabled for local cma-scale image" /usr/local/share/cmaeg/media-self-test.txt'
echo "eg_media=disabled-local-marker-verified"
if [[ "$build_only" == true ]]; then
  echo "build_only_complete source_commit=$commit api=$api_tag eg=$eg_tag platform=$platform"
  exit 0
fi

python3 - "$cluster_root/manifests/apps.json" "$manifest" "$api_tag" "$eg_tag" "$commit" <<'PY'
import json
from pathlib import Path
import sys

source, destination, api_tag, eg_tag, commit = sys.argv[1:]
data = json.loads(Path(source).read_text())
changed = set()
for item in data.get("items", []):
    if item.get("kind") != "Deployment":
        continue
    name = item.get("metadata", {}).get("name")
    if name == "cma-agent-manager":
        tag = api_tag
        changed.add(name)
    elif name == "cma-event-gateway":
        tag = eg_tag
        changed.add(name)
    else:
        continue
    item["spec"]["template"]["metadata"].setdefault("annotations", {})["cma.chronoai.dev/source-sha"] = commit
    for container in item["spec"]["template"]["spec"].get("containers", []):
        if name == "cma-agent-manager" and container.get("name") == "api":
            container["image"] = tag
        if name == "cma-event-gateway" and container.get("name") == "cmaeg":
            container["image"] = tag
if changed != {"cma-agent-manager", "cma-event-gateway"}:
    raise SystemExit("unexpected deployment set: " + repr(sorted(changed)))
Path(destination).write_text(json.dumps(data, indent=2) + "\n")
PY

k="$cluster_root/scripts/k"
kind="$cluster_root/bin/kind"
echo "mongo_wait_start=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$k" wait --for=condition=ready pod -l app=mongo --timeout=180s
mongo_ready=$("$k" get pods -l app=mongo -o json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sum(1 for p in d["items"] if p.get("status",{}).get("phase")=="Running" and any(c.get("ready") for c in p.get("status",{}).get("containerStatuses",[]))))')
mongo_replicas=$("$k" get statefulset/mongo -o jsonpath='{.spec.replicas}')
[[ "$mongo_ready" == "$mongo_replicas" && "$mongo_ready" == 3 ]] || {
  echo "Mongo is not a healthy 3-member replica set: ready=$mongo_ready replicas=$mongo_replicas" >&2
  exit 1
}
echo "mongo_ready=$mongo_ready/$mongo_replicas"

echo "kind_load_start=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$kind" load docker-image --name cma-scale "$api_tag" "$eg_tag"
echo "kind_load_complete=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$k" apply -f "$manifest"
"$k" rollout status deployment/cma-agent-manager --timeout=300s
"$k" rollout status deployment/cma-event-gateway --timeout=300s
"$k" wait --for=condition=ready pod -l app.kubernetes.io/name=cma-agent-manager --timeout=180s
"$k" wait --for=condition=ready pod -l app.kubernetes.io/name=cma-event-gateway --timeout=180s

count_ready() {
  local selector=$1
  "$k" get pods -l "$selector" -o json | python3 -c '
import json
import sys
d = json.load(sys.stdin)
ready = []
for pod in d.get("items", []):
    status = pod.get("status", {})
    ok = status.get("phase") == "Running" and any(
        c.get("type") == "Ready" and c.get("status") == "True"
        for c in status.get("conditions", [])
    )
    if ok:
        ready.append(pod["metadata"]["name"])
print(" ".join(ready))
if len(ready) != 2:
    raise SystemExit(1)
'
}
am_ready=$(count_ready app.kubernetes.io/name=cma-agent-manager)
eg_ready=$(count_ready app.kubernetes.io/name=cma-event-gateway)
echo "am_ready=2/2 pods=$am_ready"
echo "eg_ready=2/2 pods=$eg_ready"
echo "build_load_ready source_commit=$commit api=$api_tag eg=$eg_tag platform=$platform"
