#!/usr/bin/env bash
set -euo pipefail

# Run on a fresh hosted runner in the checked-out candidate and retain only result.json.
result=${1:?Supply a path for the JSON result}
source_sha=$(git rev-parse HEAD)
repo=$(git rev-parse --show-toplevel)
result=$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$result")
test -z "$(git status --porcelain=v1)"
edit=api/crates/cma-am-agent-log/src/lib.rs
evidence=$(mktemp -d "${RUNNER_TEMP:?Run on the hosted benchmark runner}/am-rebuild.XXXXXX")
trap 'git -C "$repo" restore -- "$edit"; rm -rf -- "$evidence"' EXIT

export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="$evidence/target"
export CHRONO_BUILD="git-${source_sha:0:12}"
export CHRONO_BUILD_SHA="$source_sha"

cd "$repo/api"
rust_version=$(rustc --version)
cargo metadata --locked --no-deps --format-version 1 >"$evidence/metadata.json"
build=(cargo build --release --locked --bin cma
    --config profile.release.package.cma.codegen-units=256 --message-format=json)

run_timed() {
    local phase=$1
    python3 - "$evidence/$phase.seconds" "${build[@]}" <<'PYTHON'
from pathlib import Path
import subprocess
import sys
import time

start = time.monotonic()
subprocess.run(sys.argv[2:], check=True)
Path(sys.argv[1]).write_text(str(time.monotonic() - start))
PYTHON
}

run_timed seed >"$evidence/seed.ndjson"
"${build[@]}" >"$evidence/baseline.ndjson"

python3 - "$repo/$edit" <<'PY'
from pathlib import Path
import sys

file = Path(sys.argv[1])
source = file.read_text()
text = "Agent log segment identity, format or integrity is invalid"
if source.count(text) != 1:
    raise SystemExit("Agent log diagnostic edit no longer applies exactly once")
file.write_text(source.replace(text, "Agent log segment identity, format, or integrity is invalid"))
PY
test "$(git diff --name-only)" = "$edit"
run_timed warm >"$evidence/warm.ndjson"

comparator_args=(
    --implementation cma-am-agent-log:lib:cma_am_agent_log
    --binary cma:bin:cma
    --extracted-implementation cma-am-maintenance:lib:cma_am_maintenance
)
for package in cma-am-agent-commands cma-am-auth cma-am-bindings cma-am-catalog \
    cma-am-mongo-primitives cma-am-reporting cma-am-sandbox-state cma-am-spend cma-am-triggers; do
    comparator_args+=(--fresh "$package:lib:${package//-/_}")
done
python3 -B "$repo/.github/scripts/api-release-rebuild.py" \
    "$evidence/metadata.json" "$evidence/baseline.ndjson" "$evidence/warm.ndjson" \
    "${comparator_args[@]}" >"$evidence/comparison.json"

binary="$CARGO_TARGET_DIR/release/cma"
assert_binary() {
    LC_ALL=C grep -aFq 'Agent log segment identity, format, or integrity is invalid' "$binary"
    "$binary" --print-build-info >"$evidence/$1.build-info.json"
    "$binary" --allocator-self-test >"$evidence/$1.allocator.log"
    grep -Fxq 'allocator-self-test outcome=ok' "$evidence/$1.allocator.log"
    sha256sum "$binary" | cut -d ' ' -f 1 >"$evidence/$1.sha256"
}
assert_binary warm
rm -rf -- "$CARGO_TARGET_DIR"
run_timed clean >"$evidence/clean.ndjson"
assert_binary clean

python3 - "$evidence" "$result" "$source_sha" "$rust_version" <<'PY'
import json
from pathlib import Path
import sys

evidence, result = map(Path, sys.argv[1:3])
comparison = json.loads((evidence / "comparison.json").read_text())
if comparison["extraction"]["status"] != "pass":
    raise SystemExit("The extracted reverse dependency did not qualify")
workspace = {
    package["id"] for package in json.loads((evidence / "metadata.json").read_text())["packages"]
}
clean_packages = set()
with (evidence / "clean.ndjson").open() as clean:
    for line in clean:
        event = json.loads(line)
        if event["reason"] == "compiler-artifact" and event["package_id"] in workspace:
            if event["fresh"]:
                raise SystemExit("Clean build reused a workspace compiler artifact")
            clean_packages.add(event["package_id"])
if not {artifact["package_id"] for artifact in comparison["artifacts"]["edited"]} <= clean_packages:
    raise SystemExit("Clean build did not compile every workspace package from the warm build")
for suffix in ("build-info.json", "sha256"):
    if (evidence / f"warm.{suffix}").read_bytes() != (evidence / f"clean.{suffix}").read_bytes():
        raise SystemExit(f"Warm and clean outputs differ for {suffix}")

stamp = json.loads((evidence / "warm.build-info.json").read_text())
if stamp != {"version": "git-" + sys.argv[3][:12], "source_sha": sys.argv[3]}:
    raise SystemExit("Compiled source stamp does not match the checked-out candidate")

rebuild = sorted({artifact["package"] for artifact in comparison["artifacts"]["edited"] if not artifact["fresh"]})
if rebuild != ["cma", "cma-am-agent-log", "cma-am-maintenance"]:
    raise SystemExit(f"Unexpected workspace rebuild set: {rebuild}")
fresh = sorted({artifact["package"] for artifact in comparison["artifacts"]["edited"] if artifact["fresh"]} - set(rebuild))
result.write_text(json.dumps({
    "source_sha": sys.argv[3],
    "rustc": sys.argv[4],
    "seconds": {phase: float((evidence / f"{phase}.seconds").read_text()) for phase in ("seed", "warm", "clean")},
    "rebuilt_workspace_packages": rebuild,
    "fresh_workspace_packages": fresh,
    "binary_sha256": (evidence / "warm.sha256").read_text().strip(),
    "edited_diagnostic": True,
    "build_info_matches": True,
    "allocator_self_test": True,
}, indent=2) + "\n")
PY
