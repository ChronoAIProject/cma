# Secret Scanner Build

The API image contains a **CMA security rebuild of Gitleaks 8.30.1**, not the
untouched upstream release binary. The upstream release's Go standard library
and `golang.org/x/crypto` and `golang.org/x/text` versions failed the strict image
vulnerability gate. The rebuild preserves upstream source, embedded default
rules, regex implementation, and version output while updating those build
inputs. It does not add scanner configuration, ignore entries, or policy
exceptions. The Rust scanner's bounded, fully redacted stdin protocol and exact
version check are unchanged.

## Reviewed Inputs

All artifact and dependency pins are literal values in `infra/api/Dockerfile`,
not overrideable build arguments.

| Input | Pin |
| --- | --- |
| Gitleaks version | `8.30.1` |
| Upstream commit | `83d9cd684c87d95d656c1458ef04895a7f1cbd8e` |
| Source archive SHA-256 | `6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115` |
| Embedded `config/gitleaks.toml` SHA-256 | `e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf` |
| Go toolchain | `1.26.9` |
| Go builder | `golang:1.26.9-alpine3.23@sha256:6750308561953d451210ed1a8086d36c8c8bdcbf672d38d9d0d424b8a7a0216c` |
| Resolved `go.mod` SHA-256 | `e0edfb0862afb47842aee08811984801fef09c60865c77988c814a2b58450404` |
| Resolved `go.sum` SHA-256 | `8220b35a8d9ab7e2befe8880db36aded4fc68547ce5c2b42bb28c102d933e09c` |

The source archive is downloaded from
`https://codeload.github.com/gitleaks/gitleaks/tar.gz/83d9cd684c87d95d656c1458ef04895a7f1cbd8e`
and verified before extraction. The multi-platform builder index supports both
Linux architectures accepted by the scanner stage. Current API delivery targets
Linux amd64; the web image also targets Linux arm64.

The only explicit dependency requests are `golang.org/x/crypto@v0.55.0` and
`golang.org/x/text@v0.41.0`. Go's module resolver raises the source language floor
from `1.24.11` to `1.25.0` and selects `x/sys v0.47.0` and `x/sync v0.22.0` for
existing requirements. Crypto also requires `x/net v0.57.0` and `x/term v0.45.0`;
text declares `x/tools v0.48.0` and `x/mod v0.38.0` in its graph. No blanket
dependency upgrade is performed. The exact resulting graph and checksum file
are verified against the reviewed hashes above after `go mod download all`.

## Build Boundaries

`secret-scanner-build` runs Go on `BUILDPLATFORM` and cross-compiles for
`TARGETOS` and `TARGETARCH`. It rejects every target except `linux/amd64` and
`linux/arm64`. Go settings are explicit, automatic toolchain downloads are
disabled, and module downloads use the public checksum database.

Networked preparation downloads and verifies the source and modules. Compilation
is a separate `RUN --network=none` step with `GOPROXY=off`, `-mod=readonly`,
`-trimpath`, `-buildvcs=false`, and `CGO_ENABLED=0`. The reviewed input hashes are
checked before and after compilation. The release's `-tags gore2regex` selects
`github.com/wasilibs/go-re2`; omitting that tag would change scanner semantics.
The version linker flag is exactly
`-X=github.com/zricethezav/gitleaks/v8/version.Version=8.30.1`.

`secret-scanner` is the independently buildable, target-platform stage. It runs
the rebuilt binary's version command on the target architecture and requires
`8.30.1`. Cross-platform builds need the same target execution support used by
the rest of the API image build. The final `runtime` stage copies this checked
binary without adding a compiler, module cache, or upstream source tree.

The runtime retains `/usr/local/share/gitleaks/source.txt`, `inputs.sha256`,
`buildinfo.txt`, and `binary.sha256`. These record the upstream revision, source
archive and builder pins, source-relative input hashes, linked module versions,
build settings, and binary digest. Full resolved `go.mod`, `go.sum`, and
`modules.txt` remain at `/src/gitleaks` in `secret-scanner-build` for audit; the
runtime does not ship the unused module graph as an application dependency.

These checks make source and dependency drift fail closed. They do not promise
an entirely offline cold build, perpetual upstream availability, bit-identical
API images, or a clean future vulnerability scan. The preparation step still
needs the pinned registry content, source archive, Go module proxy, checksum
database, and Alpine package repository. The strict image scan remains required
for every candidate and architecture.

## Verification

Run the offline Dockerfile contract checks from the repository root:

```sh
bash .github/secret-scanner-build-tests.sh
```

The script checks the artifact and graph pins, executes the Dockerfile's actual
architecture guard with supported and rejected inputs, checks shell syntax,
and verifies offline build flags, target-platform version checking, retained
provenance, and final API OCI labels. It does not build an image or prove binary
compatibility.

An absolute `CMA_TEST_GITLEAKS` path additionally enables real-binary CLI checks:

```sh
CMA_TEST_GITLEAKS=/absolute/path/to/gitleaks bash .github/secret-scanner-build-tests.sh
```

These checks clear the child environment, use only embedded defaults, run from
`/`, and use CMA's production stdin flags: fatal logging, full redaction, JSON
stdout, detection exit code 42, ignored inline suppression annotations, and
`/dev/null` as the ignore path. Synthetic detection values are assembled only
at runtime. The script bounds execution and report size and checks clean input,
generic and package-token detection, redaction, line mapping, and rejected
annotation bypasses. Never use real credentials as test inputs.

Before delivery, build `secret-scanner` for both supported Linux architectures
and the final API image for every deployment target; inspect the retained build
metadata and digests; run the real
CLI checks on matching native or emulated execution; run the ignored Rust test
`profile_policy::secret_scanner::tests::pinned_engine_detects_complete_rules_beyond_prefix_markers`
with `CMA_TEST_GITLEAKS`; run the repository secret scan; and rerun the unchanged
strict image vulnerability gates. A separately compiled native scanner may
exercise the Rust conformance test on a non-Linux development host, but is not
a substitute for checking the shipped Linux binaries.
