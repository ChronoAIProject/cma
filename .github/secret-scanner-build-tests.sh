#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo_dir=$(CDPATH='' cd -- "$script_dir/.." && pwd -P)
dockerfile="$repo_dir/infra/api/Dockerfile"
tests=0

fail() {
  printf 'not ok - %s\n' "$*" >&2
  exit 1
}

pass() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "$tests" "$1"
}

contains() {
  case "$1" in
    *"$2"*) ;;
    *) fail "$3" ;;
  esac
}

heredoc_body() {
  awk -v marker="$1" '
    $0 == marker { copying = 0 }
    copying { print }
    /^RUN / && index($0, "\047" marker "\047") { copying = 1 }
  ' "$dockerfile"
}

prepare=$(heredoc_body PREPARE_SCANNER)
build=$(heredoc_body BUILD_SCANNER)
[ -n "$prepare" ] && [ -n "$build" ] || fail 'scanner build steps are missing'
printf '%s\n' "$prepare" | sh -n
printf '%s\n' "$build" | sh -n
pass 'scanner build steps have valid POSIX shell syntax'

image_definition=$(<"$dockerfile")
contains "$image_definition" \
  'FROM --platform=$BUILDPLATFORM golang:1.26.10-alpine3.23@sha256:a07f231c19b03e6106cf442f02a5bcf262147e744d15b7b5e4b6d81d420c70a0 AS secret-scanner-build' \
  'native builder must use the reviewed multi-platform Go digest'
contains "$prepare" 'test "$(go env GOVERSION)" = '\''go1.26.10'\' \
  'builder must assert its Go version'
contains "$image_definition" 'GOTOOLCHAIN=local GOENV=off GOTELEMETRY=off' \
  'builder must not inherit Go settings or download another toolchain'
contains "$image_definition" 'GOPROXY=https://proxy.golang.org GOSUMDB=sum.golang.org CGO_ENABLED=0' \
  'module integrity verification and static linking must remain enabled'
pass 'builder and Go settings are pinned'

architecture_guard=$(printf '%s\n' "$prepare" | awk '/^case / { copying = 1 } copying { print } /^esac$/ { exit }')
[ -n "$architecture_guard" ] || fail 'platform guard is missing'
for target_arch in amd64 arm64; do
  TARGETOS=linux TARGETARCH="$target_arch" sh -c "$architecture_guard" \
    || fail "supported platform linux/$target_arch was refused"
done
for target_arch in '' 386 arm riscv64 unknown; do
  if TARGETOS=linux TARGETARCH="$target_arch" sh -c "$architecture_guard" 2>/dev/null; then
    fail "unsupported architecture '$target_arch' was accepted"
  fi
done
if TARGETOS=darwin TARGETARCH=arm64 sh -c "$architecture_guard" 2>/dev/null; then
  fail 'non-Linux target was accepted'
fi
pass 'actual platform guard accepts only Linux amd64 and arm64'

contains "$prepare" \
  'https://codeload.github.com/gitleaks/gitleaks/tar.gz/83d9cd684c87d95d656c1458ef04895a7f1cbd8e' \
  'source must use the exact upstream commit'
contains "$prepare" \
  "'6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115  /tmp/scanner.tar.gz' | sha256sum -c -" \
  'source archive checksum must be verified'
contains "$prepare" \
  "'e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml' | sha256sum -c -" \
  'unchanged embedded default rules must be verified'
case "$prepare" in
  *'| sha256sum -c -'*'tar -xzf /tmp/scanner.tar.gz'*) ;;
  *) fail 'archive checksum must be checked before extraction' ;;
esac
pass 'source archive and embedded rules are checksum-verified'

contains "$prepare" 'go get golang.org/x/crypto@v0.55.0 golang.org/x/text@v0.41.0 github.com/klauspost/compress@v1.18.7' \
  'dependency changes must use the exact reviewed versions'
contains "$prepare" '0d21d8af0e74395d5b88a1fd5ca4f421818c45b3f7a2b38100566eeca61ad675  go.mod' \
  'resolved module graph must be pinned'
contains "$prepare" 'c1f21f073cc203c7678f230b9cb14131a25729f3e2fdbd73475d0de44be5711e  go.sum' \
  'resolved module checksums must be pinned'
contains "$prepare" 'go mod download all' 'modules must be cached before the offline build'
contains "$prepare" 'go mod verify' 'downloaded modules must be verified'
case "$prepare" in
  *'go get -u'*|*'@latest'*|*'GOSUMDB=off'*) fail 'unbounded or unverified module resolution' ;;
esac
pass 'dependency resolution is explicit and checked against the reviewed graph'

contains "$image_definition" "RUN --network=none <<'BUILD_SCANNER'" \
  'compilation must run without network access'
contains "$build" 'export GOOS="${TARGETOS}" GOARCH="${TARGETARCH}" GOPROXY=off' \
  'offline cross-compilation must use the requested target'
contains "$build" 'go mod verify' 'offline modules must be verified again'
contains "$build" 'go build -mod=readonly -trimpath -buildvcs=false -tags gore2regex' \
  'readonly release-compatible build flags must be retained'
contains "$build" '-ldflags '\''-s -w -X=github.com/zricethezav/gitleaks/v8/version.Version=8.30.1'\' \
  'exact scanner policy version must be retained'
case "$build" in
  *'sha256sum -c /usr/local/share/gitleaks/inputs.sha256'*'go build '*'sha256sum -c /usr/local/share/gitleaks/inputs.sha256'*) ;;
  *) fail 'source inputs must be verified both before and after compilation' ;;
esac
pass 'compilation is offline, readonly, and preserves release regex/version semantics'

scanner_stage=$(awk '/^FROM alpine:3.22 AS secret-scanner$/ { copying = 1; next } copying && /^FROM / { exit } copying { print }' "$dockerfile")
contains "$scanner_stage" 'COPY --from=secret-scanner-build /usr/local/bin/gitleaks /usr/local/bin/gitleaks' \
  'target verifier must use the rebuilt binary'
contains "$scanner_stage" 'RUN --network=none test "$(/usr/local/bin/gitleaks version)" = '\''8.30.1'\' \
  'scanner version must be executed on the target platform'
contains "$build" 'go version -m /usr/local/bin/gitleaks > /usr/local/share/gitleaks/buildinfo.txt' \
  'linked module and toolchain provenance must be retained'
contains "$build" 'sha256sum /usr/local/bin/gitleaks > /usr/local/share/gitleaks/binary.sha256' \
  'binary digest must be retained'
pass 'target-platform version and binary provenance checks are retained'

runtime_stage=$(awk '/^FROM alpine:3.22 AS runtime$/ { copying = 1; next } copying { print }' "$dockerfile")
contains "$runtime_stage" 'ARG CHRONO_BUILD=development' 'final-stage version argument is missing'
contains "$runtime_stage" 'ARG CHRONO_BUILD_SHA=development' 'final-stage revision argument is missing'
contains "$runtime_stage" 'org.opencontainers.image.source="https://github.com/ChronoAIProject/cma"' \
  'final-stage source label is missing'
contains "$runtime_stage" 'org.opencontainers.image.revision="${CHRONO_BUILD_SHA}"' \
  'final-stage revision label is missing'
contains "$runtime_stage" 'org.opencontainers.image.version="${CHRONO_BUILD}"' \
  'final-stage version label is missing'
contains "$runtime_stage" 'COPY --from=secret-scanner /usr/local/bin/gitleaks /usr/local/bin/gitleaks' \
  'API image must copy the target-verified scanner'
contains "$runtime_stage" 'COPY --from=secret-scanner /usr/local/share/gitleaks /usr/local/share/gitleaks' \
  'API image must retain scanner provenance'
pass 'API image retains verified scanner, provenance, and OCI identity'

if [ -n "${CMA_TEST_GITLEAKS:-}" ]; then
  command -v python3 >/dev/null 2>&1 || fail 'python3 is required for real-binary checks'
  python3 - "$CMA_TEST_GITLEAKS" <<'PY'
import itertools
import json
import os
import subprocess
import sys

executable = sys.argv[1]
if not os.path.isabs(executable):
    sys.exit("real scanner path must be absolute")

def run(arguments, value):
    return subprocess.run(
        [executable, *arguments], input=value, text=True, cwd="/",
        env={"GITLEAKS_CONFIG_TOML": "[extend]\nuseDefault = true\n"},
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=3,
        check=False,
    )

version = run(["version"], "")
if version.returncode != 0 or version.stdout.strip() != "8.30.1":
    sys.exit("real scanner version does not match the policy pin")

arguments = [
    "stdin", "--no-banner", "--no-color", "--log-level", "fatal",
    "--redact=100", "--report-format", "json", "--report-path", "-",
    "--exit-code", "42", "--ignore-gitleaks-allow",
    "--gitleaks-ignore-path", "/dev/null",
]
alphabet = "aB3dE6gH9jK2mN5pQ8sT1vW4yZ7cF0iL"
generic = "".join(itertools.islice(itertools.cycle(alphabet), 40))
npm = "npm_" + "".join(itertools.islice(itertools.cycle(alphabet), 36))
annotation = "# gitleaks:" + "allow"
cases = [
    ("clean values", "PROJECT_MODE=development\nPUBLIC_ORIGIN=https://example.test\n", None),
    ("generic detection", "SERVICE_API_KEY=" + generic + "\n", generic),
    ("package token detection", "SERVICE_API_KEY=" + npm + "\n", npm),
    ("annotation cannot bypass detection", "SERVICE_API_KEY=" + generic + " " + annotation + "\n", generic),
]
for name, value, secret in cases:
    result = run(arguments, value)
    if len(result.stdout.encode()) > 64 * 1024:
        sys.exit(name + ": report exceeded the production bound")
    try:
        findings = json.loads(result.stdout)
    except json.JSONDecodeError:
        sys.exit(name + ": scanner did not return JSON")
    if not isinstance(findings, list):
        sys.exit(name + ": scanner did not return a findings array")
    if secret is None:
        if result.returncode != 0 or findings:
            sys.exit(name + ": clean input was rejected")
    else:
        if result.returncode != 42 or not findings:
            sys.exit(name + ": expected detection was missing")
        if secret in result.stdout:
            sys.exit(name + ": scanner report was not fully redacted")
        if any(not isinstance(item, dict) or item.get("StartLine") != 1
               or item.get("EndLine") != 1 for item in findings):
            sys.exit(name + ": finding line mapping was invalid")
PY
  pass 'real scanner preserves production stdin, redaction, and no-bypass behavior'
else
  printf 'skip - real-binary checks require an absolute CMA_TEST_GITLEAKS path\n'
fi
