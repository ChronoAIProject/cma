# GitHub Runtime Transport

The runtime image builds the genuine upstream GitHub CLI, version 2.100.0,
from commit `45437bc7eeeb3359bbfddd1742f79de7652fd3e2`. The source archive and
both architecture toolchain archives have pinned SHA-256 checksums. Go 1.26.9
builds the unchanged CLI source with two indirect dependencies updated:
`golang.org/x/mod` 0.40.0 fixes CVE-2026-56864 and CVE-2026-56865, and
`golang.org/x/net` 0.60.0 fixes CVE-2026-78669, which remain in the upstream
release binary. Go's checksum database and module verification remain enabled;
the final build uses readonly module resolution. Image evidence records the
installed executable digest and verifies the version. This is a
dependency-refreshed upstream build, not the upstream-published binary; neither
CLI transport code nor GitHub credentials are patched into it.
The Linux image campaign also runs the installed CLI through the actual
supervisor proxy against an offline fixture with networking disabled.

When `CHRONO_SANDBOX_REACHES` includes exactly `api-github`, the runtime prepares
a local Unix-socket proxy after software installation and credential admission.
The proxy forwards GitHub API paths to
`CHRONO_PLATFORM_BASE_URL/api/v1/proxy/s/api-github`. The scoped NyxID credential
comes from the existing in-memory credential holder and follows its custody
generation checks. Requests before credential admission cannot use the proxy.

The supervisor sets `GH_CONFIG_DIR`, `GH_HOST`, `GH_TOKEN` and
`GH_PROMPT_DISABLED` for Workspace setup and Agent processes. Agent plans cannot
override those variables when the proxy is enabled. `GH_TOKEN` is an inert
placeholder, not a GitHub or NyxID credential. The generated configuration lives
under the runtime state directory, outside the snapshotted Sandbox home; it
contains only the socket path and HTTPS Git protocol preference. Neither the
configuration nor shared profiles set a Git author, email or account name.

The socket only accepts `api.github.com` requests, including REST and GraphQL.
Foreign hosts, conflicting absolute destinations, tunneling requests and paths
escaping the configured service are refused. Upstream redirects are not followed
by the credential-bearing client. Caller authorization and provider routing
headers are replaced or stripped, and the existing response guard refuses
credential-bearing headers and bodies. Configuration directories are private,
configuration files and the socket are owner-only, and a process lease prevents
replacing an active socket. Restart may recover a stale socket but does not
delete unrelated entries occupying its path.

## Scope And Acceptance

Installing the runtime image does not grant a new service to existing Sandboxes,
change pinned profiles, or rotate an active owner's runtime. NyxID account
connection and Sandbox service authorization are distinct prerequisites.

Local regression tests cover proxy routing, exact request preservation,
credential renewal, leak refusal, stale socket recovery, active-socket protection
and the genuine CLI's REST and GraphQL transport. Run the executable contract
test explicitly with the pinned `gh` binary:

```sh
CMA_GH_CONTRACT_EXECUTABLE=/absolute/path/to/gh \
  cargo test --locked -p layer4-runtime github_proxy::tests::genuine_cli \
  -- --ignored
```

This transport is not a general-purpose network proxy. Native Git subprocesses
do not inherit `gh`'s HTTP Unix-socket transport, and the inert placeholder cannot
authorize native Git pushes. GitHub contribution acceptance therefore requires
the explicitly authorized Sandbox grant, tested GitHub operations through `gh`,
and the Trigger Connect/Verify flow; REST/GraphQL transport alone is not full
contribution acceptance. Production deployment and verification are recorded in
`CMA_Unattended_Acceptance.md` separately from local test results.
