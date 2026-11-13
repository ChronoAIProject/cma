# Local validation for #1391

Validated on November 6, 2026 in the dedicated `tl/cma-desktop-browser/integration`
worktree. No remote CMA Sandbox or Agent was created and production was not changed.

## Results

| Check | Result |
| --- | --- |
| Original Omarchy launcher with the regression fixture browser | Reproduced: launcher and output pipes remain open while browser lives |
| Linux launcher regressions | 11 passed, including both desktop branches, pipe EOF, session isolation, concurrent reuse, failure, timeout, and owner SIGKILL cleanup |
| Desktop recovery source/ownership tests | 13 passed, including retained XFCE/Openbox/Omarchy versions |
| Recovery suite on native Linux in both runtime image builds | 40 passed per image |
| Rust 1.96.1 format and affected-crate Clippy, all targets/features, warnings denied | Passed |
| CMA crate tests with CI's 32 MiB test-thread stack | 1,248 passed; 678 existing tests ignored |
| Adapter crate tests | 627 passed |
| Runtime crate tests, serial execution | 335 passed; 3 existing tests ignored |
| Runtime pool-assignment integration test | 1 passed |
| Adapter doctest command | Passed; no doctests declared |
| ShellCheck and shell/Python/JavaScript syntax checks | Passed |
| Source-size and diff-whitespace checks | Passed |
| Release/provider workflow contracts and image metadata checks | Passed |
| Runtime image evidence/provenance unit tests | 17 passed |
| Native arm64 Debian desktop and default runtime image builds | Both passed all built-in checks |
| Real desktop/native-tool image contract, including retained-home restart | Passed twice; completed tools and ACP `end_turn` observed with the page still open |
| Default runtime protocol/bootstrap contract on native arm64 | All 36 checks passed; exact container/volume cleanup passed |

The serial runtime run avoids contention between existing writer-lock tests in
one `cargo test` process. CI uses nextest, which isolates those tests. The first
CMA run omitted CI's `RUST_MIN_STACK=33554432` and overflowed an existing large
continuation test; the run with the CI setting passed.

## Architecture limits

The CI-target `linux/amd64` desktop image compiled and passed its 42 native
provider tests under Docker Desktop emulation, but its existing Linux recovery
prerequisite failed: `os.pidfd_open` returned `ENOSYS`. The identical prerequisite
passes on native arm64. The amd64 image gate therefore has **not** passed locally;
it needs a native amd64 runner. The tech lead approved leaving that gate to CI
after the other local gates passed and will monitor the amd64 Images job.

Both arm64 images used the repository Dockerfile, with only a temporary
`CARGO_BUILD_JOBS=4` build argument added to bound the compiler workers. No image
build tests were removed. The desktop image evidence script ran unchanged.
The default image evidence script intentionally admits only amd64. A temporary
local copy selected arm64 admission, the existing arm64 native checksum, and the
arm64 binary/alias paths; all 36 protocol/bootstrap assertions were retained.
The checked-in release validator still requires amd64.

The Omarchy launch branch and Hyprland readiness/focus path passed the Linux fake
browser tests. The real Omarchy image was not executed locally; its existing
native-tool probe now includes the same close/reopen/reuse and terminal-turn
assertions for execution on its supported image runner. The offline image probes
verify the real provider terminal boundary, not deployed CMA REST or NyxID state.

## Release scope

Ship the API image with the new recovery source pins and setup guidance before
or together with the runtime images. Rebuild the Debian desktop runtime
(`RUNTIME_DESKTOP=1`), the Omarchy runtime target, and the normal default runtime
that also packages the shared launcher. Existing retained launcher hashes remain
accepted during the rollout. No response-completion inference was added.
