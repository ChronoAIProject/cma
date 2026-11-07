# GitHub Runtime Transport

The runtime image builds the genuine upstream GitHub CLI, version 2.100.0,
from commit `45437bc7eeeb3359bbfddd1742f79de7652fd3e2`. The source archive and
both architecture toolchain archives have pinned SHA-256 checksums. Go 1.26.10
fixes CVE-2026-111943 in crypto/x509 and builds the unchanged CLI source with
two indirect dependencies updated:
`golang.org/x/mod` 0.41.0 fixes CVE-2026-56864 and CVE-2026-56865, and
`golang.org/x/net` 0.62.0 fixes CVE-2026-78669 and CVE-2026-78661, which remain
in the upstream release binary. Go's checksum database and module verification
remain enabled; the final build uses readonly module resolution. Image evidence
records the installed executable digest and verifies the version. This is a
dependency-refreshed upstream build, not the upstream-published binary; neither
CLI transport code nor GitHub credentials are patched into it.
The Linux image campaign also runs the installed CLI through the actual
supervisor proxy against an offline fixture with networking disabled.

Every runtime configured with the managed platform proxy prepares a local
GitHub Unix-socket transport after software installation and credential admission,
including the default `chrono-llm-public` and `ornn-api` scope. The transport does
not grant GitHub access: NyxID checks each request against the current agent key.
An explicit hot scope grant or removal therefore changes access on the next
request without changing the running Agent's environment or restarting it.
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

## Agent source setup

New inline Agent Manager admissions capture versioned GitHub instructions in
their durable lifecycle input. The worker composes those captured bytes with
the user's instructions when creating the immutable Agent profile. The existing
home renderer writes that profile to the native Agent's `AGENTS.md`, so a later
request to clone a repository receives the same guidance after conversation
compaction or reload. This covers new Sandboxes and new Agents in existing
Sandboxes/Workspaces. It does not rewrite previously created Agent homes or
historical profiles, and does not change separately authored Trigger/Bot profiles.

The Agent uses `gh api` to resolve the requested ref, or the repository's
`default_branch` when no ref was supplied, pins the commit, and downloads the
API tarball. NyxID follows the archive redirect server-side; Git Trees/Blobs or
Contents at the pinned commit are the fallback if the archive is unavailable.
The Agent validates and stages the archive, preserves the current worktree and
initializes a local Git repository only when the destination is not already in
one. Repo creation and requested publication use GitHub APIs as well. The setup
message requests this work; CMA does not execute a programmatic clone/download.
The detailed API recipe and boundaries are in
[`CMA_GitHub_API_Workflow.md`](CMA_GitHub_API_Workflow.md).

This change requires an Agent Manager/API image rebuild only. It uses the
existing `gh` transport, home-file delivery and native instructions mechanism;
the runtime image, NyxID services, credentials and scopes do not change.
Admission keeps the submitted request unchanged for idempotency. Older operations
without captured instructions reconstruct their original profiles. New prepared
capacity pins the exact same managed instructions as new default admissions;
historical empty-instruction preparations remain readable and replayable. The
normal desired-plan reconciliation replenishes unclaimed capacity with the new
profile; owned Sandboxes retain their recorded profiles and runtime identities.
Prepared replay hashes the recorded prompt rather than substituting empty text.

Retain a compatible Agent Manager when rolling back after new admissions: an
older binary ignores the captured-instructions field and rejects new prepared
profiles with nonempty instructions. Reverting UI or the default runtime image
does not require reverting this compatible reader. Local tests prove admission
capture, immutable profile/home replay, current and historical preparation, and
instruction delivery; production source-download acceptance remains a separate
Agent exercise against an authorized repository.

## Scope And Acceptance

Installing the runtime image does not grant a new service to existing Sandboxes,
change pinned profiles, or rotate an active owner's runtime. NyxID account
connection and Sandbox service authorization are distinct prerequisites.

Local regression tests cover proxy routing, exact request preservation,
credential renewal, hot grant and removal from the default scope, leak refusal,
stale socket recovery, active-socket protection and the genuine CLI's REST and
GraphQL transport. Grant and removal keep the same socket and Agent environment;
the upstream authority refuses the initial and removed scopes. Run the executable
contract test explicitly with the pinned `gh` binary:

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
