# Protected CI Contracts

This is a deliberate CI policy migration from the protected checks at develop
`d7399c9986d1c892d375d4e42aafa582193bd681`. It must be reviewed in its own PR,
as required by CLAUDE.md section 9. Retaining the two check names does not make
their old and new meanings identical. No branch protection setting is changed
by this work, and no manual status, unconditional-success job, or merge bypass
is an acceptable substitute. Normal backend, frontend, SDK, secret, image,
manifest, and structure gates remain mandatory.

## What The Checks Execute

`Trigger release contract` executes the release/Gateway source validator,
workflow mutation regressions, and runtime evidence/provenance regressions.
`Images (runtime)` separately executes the real built-image campaign. The
publisher's `Runtime image evidence` job validates the exact runtime export
metadata and archive checksum, pulls its immutable source reference, executes
that image, retains the bounded result, and requires a passing result. Tag and
release creation depends directly on that job. A contract test's own success
is not a runtime execution result.

`Trigger provider smoke contract` executes the remote workflow authority
validator and mutation regressions, independent-observer/receipt regressions,
e2e typechecking, fixture and cleanup Vitest tests, exact Playwright discovery
inventory validation, and local browser guard mechanics. It uses no provider,
NyxID, staging fixture, or deployment credential. Neither local mechanics nor
discovery is evidence that a remote journey ran.

The normal Backend job also runs a disposable, pinned MongoDB replica set.
Only that new container's loopback address is used. Caller Mongo URIs are
discarded. The selected ignored tests cover legacy schema custody, durable
Launch admission/progress, checkpoint publication/deletion ambiguity, and
writer-capture authority; failpoint cases run serially against the same
disposable replica set. The test runner fails on an empty selection, and
cleanup removes only containers carrying the generated exact run label.

## Backend Shutdown Evidence

Backend Bash steps source `backend-resource-env.sh` through `BASH_ENV`.
The hook starts one optional sampler per step; descendant shells inherit its
marker. It disowns that child so a caller's bare `wait` cannot wait for telemetry.
Disowning retains the shell as the process parent. It does not wrap commands,
install traps, change shell options, wait
for telemetry, or change gate order, timeouts, assertions or exit status.
The provider contract checks the hook and runs its failure-path regressions.
This dedicated CI change diagnoses unexplained runner shutdowns; it does not
change resource limits or duplicate the archive/job split proposed in #1394.

`[backend-resource]` JSON lines are offered directly to the current step log at
startup and about every ten seconds. They include UTC and elapsed step time,
memory/swap KiB and swap I/O page counters, disk bytes/free inodes, load
averages, the ten largest process RSS values in KiB, and root cgroup v2 memory
counters when available. Processes expose only numeric PIDs and kernel names
from `/proc/PID/comm`; arguments and environments are never read. JSON escapes
names so they cannot become workflow commands. Unsupported or failed probes
report `unavailable` without exception details. Sampling failures cannot set
the gate result. Before probing, the sampler installs Linux `PR_SET_PDEATHSIG`
with `SIGKILL`, then checks the expected parent again to close the startup race.
Parent death terminates the sampler independently of probe/output progress;
polling between samples is not its lifetime guarantee. If protection cannot be
installed (including non-Linux systems), sampling is disabled.

Output uses a new open file description from `/proc/self/fd/1`, opened with
`O_NONBLOCK`, and closes the sampler's inherited stdout descriptor. It never
changes flags on the gate's shared stdout. Only FIFO log output is supported;
other output types disable sampling. Each record is one atomic write bounded
by `PIPE_BUF`; full pipes, oversized records, and output errors discard the
sample without queues, retries or blocking flushes. A stalled probe may lose
subsequent samples, but cannot defer kernel parent-death termination. Real
Linux regressions exercise full pipes, a FIFO-blocked probe, descriptor EOF,
process death, unchanged gate status/stdout flags, and bare `wait`. A runner-wide
termination may prevent a final sample.
Action steps without a Bash shell are not sampled by this hook.

Samples show observations, not automatic diagnoses. RSS may include shared
pages, root cgroup counters have their own scope, and a ten-second sample can
miss a short peak. A missing sample or an exit-143 runner shutdown alone does
not prove OOM, disk exhaustion, or manual cancellation. Compare passing and
failed timelines before proposing a fix. The Image Manager step's existing
ten-minute timeout remains a distinct failure mode and is unchanged.

## Historical Guarantee Mapping

| Historical guarantee | Current disposition and evidence |
| --- | --- |
| Exact owned API/app routes, HTTP redirect, readiness policy | Ported to `gateway-source-contract.rb`, including exact metadata and the two 660-second backend policies. Duplicate objects and wrong readiness are rejected. |
| Runtime campaign mandatory for CI image success and GitHub Release creation | Ported to the real contract-3 image campaign and direct publisher dependency. Missing evidence, failed probes, and incomplete cleanup cannot pass. |
| Source revision, published OCI root, archive checksum, installed artifacts | Ported to exact export metadata validation, immutable pull and `RepoDigests` association, local execution image identity, and installed launcher/native/ACP hashes and versions. Source labels are validation, not independent writer-history authority. |
| Exclusive package writer and reviewed package-write actions | Retained by workflow permission and action-SHA checks plus existing release image reconciliation/export/import tests. Actual GHCR ACL and package-history evidence remains an external release obligation. |
| Daemon-writer checkpoint coherence, retained history, exact resume authority | Ported to a real installed-image daemon writer: hold freezes it, replay matches, wrong-token and concurrent mutation are refused, downloaded archive hash matches, resume restarts writing, and restore recovers retained history. This is not live model continuation or provider rotation proof. |
| Honest failure and cleanup evidence | Runtime runner begins with an inconclusive record, records generic non-secret failure, performs exact-label cleanup, and accepts pass only with zero remaining containers and volumes. Workflow uploads are exact-file and fail on absence. A job failing before the runner can initialize has no valid record and cannot release. |
| Provider workflow explicit consent, protected review, source and image identity before credentials | Ported to dispatch-bound commit/verifier/config hashes, live environment-policy checks, exact checkout, and independently observed deployment identity before dependency or auth-fixture execution. |
| Provider object settlement and complete evidence | Ported to complete case inventory, per-run/owner/resource custody, ordered cleanup receipts, and before/after independent deployment attestation. Pending external-account cleanup is still a hard refusal. |
| Dedicated Trigger kernel, separate provider namespace/key, ephemeral Kind deployment and bootstrap | Retired architectural mechanisms. Contract 3 uses the shared Sandbox model and scoped lifecycle authority; the deleted kernel-specific namespace/RBAC/bootstrap is not a current deployment interface. The replacement does not advertise a Kind campaign. |
| Old D-09 `cma-trigger` permission-profile/listener layout and gateway-probe command | Retired exact implementation assertions, because those commands and listener topology are absent from contract 3. The local campaign does not claim their old network-denial evidence. Provider/model isolation capabilities still require evidence against the current pinned implementation. |
| Live Gateway desired/effective graph, TLS sanitization, Cloudflare behavior, sealed custody, CAS adoption/rollback and coherent external drain | Unresolved live release/deployment obligations, not retired guarantees. The replacement source checker and Kubernetes observer do not prove the old full graph or custody protocol. Release automation has no deployment environment and is forbidden from acquiring a live infrastructure command path. No passing local check authorizes Gateway mutation. |
| Fresh signup, cross-owner isolation, real provider/model continuation and rotation | Remain full-campaign obligations. Owner-only evidence cannot advance them. This delivery does not provision identities, alter NyxID, or manufacture missing observations. |

## Image Identity

An OCI root digest, selected platform manifest digest, configuration digest,
and Docker's local image ID are separate facts. Docker's local `.Id` is opaque:
depending on the storage backend it can name a manifest instead of a config.
The local runtime record preserves Docker's `Id` only as its execution identity.
For release execution, its separate `sourceReference` is the verified exported
OCI root reference, which must appear in the inspected image's `RepoDigests`.
The campaign starts that inspected immutable local ID, not the mutable tag.

The independent deployment observer records root, platform, configuration, and
running Kubernetes image identities separately. It must validate the raw bytes
and descriptor associations, not equate these digests or trust labels alone.

## Independent Observer Authority

Remote mutation requires these explicit dispatch inputs in addition to the
exact HTTPS target and dedicated fixture owner:

- `reviewed_sha`: the exact current develop commit, also the dispatch event SHA.
- `verifier_sha256`: the SHA-256 of the installed reviewed observer program.
- `observer_config_sha256`: the SHA-256 of its reviewed target configuration.

The hosted planning job exposes these identities before protected approval.
The protected job uses the exact planned checkout, verifies that the live
`staging-e2e-mutation` environment requires reviewers, prevents self-review and
admin bypass, and permits exactly the `develop` branch. It never changes those
settings. Missing policy or access to inspect it fails closed.

Protected environment variables `CMA_ATTESTATION_VERIFIER` and
`CMA_ATTESTATION_CONFIG` identify outside-checkout, owner-only, non-symlink
files. The verifier is the reviewed standalone `deployment-observer.mjs`;
there is no automatic installation or default authority. Both files are
hash-checked before and after execution. Missing setup is a prerequisite
failure, not permission to use operator-declared fixture provenance.

The verifier receives no credential values or command-line targets. Its stdin
is a closed `cma-staging-attestation-request-v1` JSON object with a fresh UUID
nonce and `expected` containing `origin`, `commit`, `nyxidCommit`, and the
immutable `api`, `web`, and `runtime` image references. Its environment contains
only a fixed tool search path and `CMA_ATTESTATION_CONFIG`. Fixture metadata is
read to construct that expected identity, but browser authentication state and
hook credentials are not opened before successful attestation.

The reviewed config names explicit kubeconfig/context/namespace, exact
deployments and container/service identities, ConfigMap runtime/origin keys,
route/parent identity, runtime architecture, and a separately scoped NyxID
deployment. `tools.kubectl` and `tools.skopeo` each carry an absolute real
executable path and SHA-256; the config also identifies the private registry
auth file. Tool bytes and private-file safety are validated by the observer.
No ambient current Kubernetes context is authority.

The closed `cma-staging-attestation-v1` receipt contains `nonce`, `observedAt`,
`origin`, `commit`, `nyxidCommit`, `runtimePin`, and three image observations.
Each image observation names `reference`, `rootDigest`, `platformDigest`,
`configDigest`, `source`, and `revision`. API/web additionally report the actual
`runningImageID`; runtime reports the configured image, not a running Sandbox.
The observer must obtain those facts from explicit-context Kubernetes reads
and independently hashed registry bytes. The protocol's name does not itself
classify a target as staging or authorize access to any environment.

## Full Campaign Closure

The caller verifies both pre-campaign and post-campaign receipts. Each must
match its own nonce and exact expected identity, use a distinct challenge,
have the correct source/revisions, and agree on observed images. The final
receipt is at most five minutes old; the initial receipt can cover the bounded
three-hour campaign plus a five-minute closure allowance. Time cannot move backward.

All 32 current discovered cases must appear exactly once and pass, including
all six injected worker faults and all four embed types. PR discovery checks
bind that inventory to Playwright's actual test collection. An empty, partial,
duplicate, skipped, or owner-only run is refused.

Every created-resource journal identity must name a valid run, owner, kind,
resource and timestamp, and its settlement must follow its creation. Cleanup
failures, ambiguous state, unknown actions, and external-account cleanup debt
cannot be discharged by an arbitrary completion row. The current signup
journey records external account cleanup as required; this CI change does not
implement or authorize an account reconciler. A separately authorized,
authenticated cleanup proof is required before that journey can close fully.

Only a successful full audit permits upload of the curated results, cleanup
journal, independently verified attestation, existing provenance declaration,
and credential-screened screenshots. The original `operator_declared` record
is preserved as such; it is never rewritten to pretend it was independent.
Failure and cleanup debt remain failures even when some useful owner-only or
local evidence exists.
