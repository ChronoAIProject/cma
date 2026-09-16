# Guarded terminal compatibility with gVisor

The retained Codex 0.147.0 Linux sandbox uses system bubblewrap to create a
separate user, PID and network namespace, establish filesystem mounts, then
apply the native restricted-network seccomp filter. On the observed gVisor
runtime, the fresh network namespace already contains `127.0.0.1/8` on `lo`.
Bubblewrap's exclusive `RTM_NEWADDR` request therefore returns `EEXIST` before
the command starts. Its error formatter can print a stale `errno`, observed as
`bwrap: loopback: Failed RTM_NEWADDR: No child processes`.

An owned-runtime diagnostic confirmed a distinct fresh network namespace with
the existing loopback address, `EEXIST` for the exclusive request, success for
the identical request without `NLM_F_EXCL`, and success for the following
`RTM_NEWLINK` operation. This also matches
[gVisor issue 13438](https://github.com/google/gvisor/issues/13438) and
[bubblewrap issue 745](https://github.com/containers/bubblewrap/issues/745).

The runtime build removes only `NLM_F_EXCL` from that address request. It keeps
the isolated namespace, the exact loopback address, acknowledgement handling,
link setup and every setup error check. It changes no Agent permission profile,
approval behavior, provider version, Kubernetes permission or network policy.

The build pins Debian's `0.12.0-1~deb13u1` source manifest and both source archives
by SHA-256, applies all Debian quilt patches, and retains SELinux support,
Debian's PIE/bind-now hardening and `assume_kernel=5.10.0`. That security release
includes the symlink traversal fix for GHSA-pxhw-h44j-8pfx and removes setuid
support. The hardened `openat2` path is retained. The patched executable is
installed after the final OS security refresh, and installation fails unless
the installed Debian package still matches the reviewed source version.

The image build compiles the real pinned `network.c` with deterministic netlink
replies. The original code must fail the preconfigured-loopback regression;
the patched code must pass all nine checks, including fresh-loopback setup,
link setup and rejection of permission errors, unexpected conflicts, invalid
reply identity and transport errors.

For an explicitly selected diagnostic runtime,
`.github/scripts/bubblewrap-native-probe.py --bwrap /path/to/reviewed/bwrap
--bwrap-sha256 <reviewed-digest>` enters the pinned Codex native sandbox helper
directly. It creates only temporary fixtures and makes no model request or
Agent-home/configuration access. Both read-only and workspace-write probes
require distinct network namespaces, only a loopback interface, native socket
denial, `no_new_privs`, read-only protected paths, and a positive workspace-write
control. Successful source tests alone do not prove that a deployed runtime
passes this native probe. The probe also fails on later sandbox setup errors;
it does not downgrade filesystem enforcement to accommodate them.

A rebuilt image does not replace an already running Sandbox. Runtime delivery
and an independently verified supported recovery or new allocation are separate
steps. Live Agent terminal acceptance remains separate from this no-model probe.
