# API and Event Gateway allocator qualification

Memory qualification runs in the executable that will ship:

```sh
cmaeg --allocator-self-test
cma --allocator-self-test
```

It transfers bounded batches of small allocations from one producer to 18
persistent workers, including aligned allocations and reallocations. Every
worker drops its batch before acknowledging completion. RSS is sampled only
after all acknowledgements and a zero-live-payload check. After 256 warmup
rounds, the remaining fixed schedule must stay within 24 MiB of the warm RSS
baseline. The test performs 4096 rounds, has a 120-second deadline, and fails
when procfs RSS is unavailable. It uses no credentials, database or network.
The final worker joins and zero-live-payload check are mandatory. The final RSS
sample must also meet the same retention bound, and completion (including
startup and worker shutdown) must remain within the deadline.

Both services use the same std-only `cma-allocator-qualification` library. It
does not select an allocator: the API exercises its existing global mimalloc,
while the gateway exercises its existing system allocator. The API command runs
before Tokio, configuration, metrics, serving or database initialization. Extra
arguments are refused with exit 2; measurement or workload failures exit 1.
Unsupported platforms fail rather than skip qualification.

`infra/api/Dockerfile` executes `/out/cma --allocator-self-test` with networking
disabled after compilation and before the runtime copies that exact ELF. A local
API packaging route must preserve this gate on the compiled executable and run
it again on the final image before qualification PASS or image save/publication.
Use network-none, read-only, capability-drop and no-new-privileges flags with a
130-second outer timeout. Require compiler-to-runtime ELF equality and retain
the API's source labels, UID 10001, entrypoint, CA and auxiliary-tool checks.
Static Rust-to-mimalloc routing checks remain necessary but cannot replace
execution. An API self-test failure does not authorize changing the workload,
retention threshold, allocator or production memory limits.

The normal gateway Dockerfile runs this gate before its existing sandbox and nine-case
media gates. An alternative packaging route must execute all three gates on
the exact final executable and retain their output. A static musl ELF check
alone does not identify which malloc implementation the linker selected.

Release images use the official Dockerfile's `rust:1.96.1-alpine` toolchain,
`apk add --no-cache musl-dev`, and `cargo build --release --locked --bin cmaeg`.
For linux/amd64, use the x86_64 compiler and Alpine musl linker from that image.
Do not replace the Rust build stage with an unqualified cross-compiled ELF.
`cargo zigbuild` can replace libc's allocator even when the resulting ELF is
reported as static musl. Passing a memory sample does not authorize that build
route: the release contract also requires the official native Cargo build and
the runtime copy from its qualified output. Local preview packaging is not a
release target.

Local release packaging of a separately compiled official Alpine ELF must
generate its Dockerfile from that exact source checkout on every attempt:

```sh
python3 "$source/.github/scripts/prepare-cmaeg-release-dockerfile.py" \
  --source "$source" --output "$context/Dockerfile"
```

The caller must stop on a nonzero exit (`subprocess.run(..., check=True)` or
`set -e`), before running Docker or saving an image. The prepared context keeps
the existing `native/cmaeg`, qualified `dependencies/`, and committed media
fixtures under `api/crates/cmaeg/tests/fixtures/media/`. The generator replaces
only compilation and dependency preparation; it carries the selected source's
allocator and sandbox checks, complete media stage, and final runtime stage.
Missing or changed mandatory gates are errors. There is no skip switch and no
fallback to a historical prepared Dockerfile. The runtime and media stages both
copy the allocator-qualified `/out/cmaeg`; a gate failure fails the image build.
Compilation remains the ordinary locked offline Cargo build in the official
Alpine builder. The generator does not establish compiler provenance.

Final local image qualification must also execute `--allocator-self-test` on the
candidate image, using the same network-none, read-only, capability-drop and
no-new-privileges flags as the existing sandbox check. Allow 130 seconds for
container startup plus the self-test's internal 120-second deadline. A nonzero
exit or outer timeout prevents qualification PASS and image save/publication.
Keep the existing equality check between the compiled ELF, context input and
final `/usr/local/bin/cmaeg`, as well as sandbox, all nine media cases and runtime
identity checks. Historical release bundles are evidence, not future recipes.

Record the source and Cargo.lock hashes, builder index and platform image
identities, Rust/Cargo/cc/musl versions, libc archive hash, executable hash and
exact-image gate logs. CPU instruction emulation does not change which compiler
and linker are used; record it explicitly. Execute the sandbox and media gates
on an x86 Linux kernel: process translation on an ARM kernel does not establish
the required syscall behavior.

A compiler or linker change requires an equal-work retention comparison and an
application replay against the existing image from the same application source.
Before production use, the corrected image must also pass a 30-minute local
soak whose RSS slope is below 1 MiB/minute. Retain the raw samples, traffic and
history configuration, failures and toolchain receipts. No memory-limit increase
substitutes for this verification.

This guard addresses issue #2055. The failed batch 9 executable used Zig
0.16's `SmpAllocator`, whose small freed blocks remain in per-thread slab
freelists. A bounded cross-thread allocation/free probe retained growing RSS
after all payloads were freed; the same work linked to Rust's bundled musl
remained bounded. Application workload and a 30-minute image soak are separate
release requirements. This is not a claim that the bundled musl bytes match
the predecessor Alpine toolchain, or that passing a synthetic bound establishes
application capacity.
