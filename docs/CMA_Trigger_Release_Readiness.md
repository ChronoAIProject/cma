# CMA Trigger release readiness

This is the evidence ledger and public-exposure gate for issue #673 and the
`cma-trigger` milestone. It does not replace the product contract in
[CMA_Trigger.md](./CMA_Trigger.md), the recovery contract in
[CMA_Trigger_Recovery.md](./CMA_Trigger_Recovery.md), or the deployment runbook
in [CMA_Trigger_Operations.md](./CMA_Trigger_Operations.md).

The feature may be implemented and merged while live evidence is collected in
parallel. It must not be described as ready for public exposure, and the
milestone must not be closed, until every required row below has a reviewable
evidence link and a passing result. An empty link, a local transcript, a mock
provider, or prose saying a check was performed is not completion.

## 1. Evidence classes

Evidence classes are deliberately not interchangeable:

| Class              | Environment                                                                                                                        | What it can prove                                                                                                                                         | What it cannot prove                                                                                        |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `D` deterministic  | Hermetic unit, contract, integration, browser, and fake-provider tests in ordinary CI                                              | CMA state transitions, validation, idempotency, fencing, redaction, projections, exact fake-effect counts, and fixture parsing                            | GitHub/NyxID behavior, OAuth scope, provider refresh/revocation, deployed routing, or runtime continuity    |
| `P` provider       | A gated, non-production test using real GitHub and NyxID with disposable credentials and a public synthetic repository             | Wire compatibility, real OAuth/service behavior, provider response semantics, numeric identity, rate-limit/refusal handling, and external resource counts | Production deployment behavior or a control-plane/runtime replacement unless the test actually performs one |
| `S` staging/canary | An authorized deployed environment with the actual control plane, runtime image, storage, NyxID service, and rollout configuration | Restart, replacement, expiry, revocation, checkpoint, routing, scheduler, alert, and continuity behavior for that exact build/configuration               | Behavior of a different build, configuration, provider account, or production environment                   |
| `M` maintainer     | A linked review or explicit recorded decision by the accountable role                                                              | Approval of residual risk, policy, security posture, licensing, and rollout                                                                               | Executable behavior                                                                                         |

A `D` result may unblock code review. Only the required `P`, `S`, and `M`
results unblock public exposure. A provider test is not made deterministic by
retrying until it passes. A failed or flaky provider run remains visible, names
an owner, and links its successful replay only after the underlying cause is
understood.

### Dated live baseline

As of 2026-08-28:

- production serves CMA API/web `v0.7.0` and advertises only the core and
  terminal conformance classes; it has no Trigger surface and cannot provide a
  Trigger smoke or rollout result;
- a read-only probe through `chronoai-shining` proved that NyxID's catalog-ID
  route plus the exact `_nyxid_via` UserService ID reaches fixture repository
  `1349237845`, while placing that UserService ID directly after `/proxy/`
  returns a catalog-mismatch `400`. This establishes transport wiring, not a
  Launch, session-side capability, restart, expiry, revocation, or pull-request
  result. The same maintainer grant includes broad `repo` and is deliberately
  ineligible under the public-MVP scope ceiling;
- `ChronoAIProject/cma-trigger-fixture` is provisioned as a public, forkable,
  synthetic fixture at repository ID `1349237845` / node ID
  `R_kgDOUGu8VQ`, pinned by `fixture-v1` to
  `878dcb6288f94721de6724899ea0680e1086d511`; its dedicated disposable
  publisher, contributor, and NyxID service identities are not yet provisioned;
- the protected `cma-trigger-smoke` environment exists at environment ID
  `20761349259` with administrator bypass disabled, self-review prevented, the
  `AI Application` team as reviewer, and protected-branch restriction; it has
  no candidate provenance variables or disposable CMA credential yet; and
- no production deployment or restart is authorized solely to collect this
  evidence.

This baseline explains why all provider/staging rows remain open. Replace it
with linked evidence from the actual candidate build rather than reinterpreting
the existing production service as a test target.

## 2. Evidence record

Every provider, staging, canary, or drill attachment must record:

```text
evidence_id
build commit and runtime image digest
environment and configuration revision
UTC start and end
operator and independent reviewer, including safe numeric provider account IDs
numeric upstream, fork, owner, and contributor account IDs where the provider supplies them; typed Launch/session IDs and a SHA-256 fingerprint for an opaque protected service ID
fixture revision and source SHA
expected result and actual result
before/after external resource counts
safe Launch and provider correlation identifiers
fault injection or rollout action used
redaction performed
workflow/run URL and retained artifact URL
pass, fail, inconclusive, or blocked result
```

`inconclusive` is not `pass`. Record it when the harness itself prevents the
claim, such as a local network sandbox failing before a request reaches the
provider. Evidence must never contain OAuth access/refresh tokens, agent-key
values, cookies, authorization headers, prompts, repository file contents or
paths, object-store keys, provider bodies, or downloadable diff contents.

Use one evidence ID in the issue, workflow, saved dashboard interval, activity
record, and drill transcript. A safe correlation is useful; raw request or
response bodies are not.

## 3. Release ledger

The initial status of every row is `open`. Change a row to `pass` only in the
release pull request or issue record that also supplies its evidence links.
Local execution without a stable review URL may be useful during development
but does not advance this ledger.

| ID     | Class | Required evidence                                                                                                                                                                           | Exit condition                                                                                                                                                                                                                                                                                            |
| ------ | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `D-01` | D     | Trigger/revision, anonymous preview, connection, Launch review/Start, progress, workspace/diff, contribution, pull-request, cancellation, recovery, and support OpenAPI/REST contract tests | Default CI links a passing API/conformance run and proves every route is in the published contract                                                                                                                                                                                                        |
| `D-02` | D     | TypeScript and Rust public fixtures and client behavior for every Trigger route                                                                                                             | Both SDK suites, fixture parity, builds, and documentation checks pass on the final commit                                                                                                                                                                                                                |
| `D-03` | D     | Browser publisher, preview, OAuth-return review, durable progress, session link, contribution review, and PR confirmation flows                                                             | Frontend unit/rendered-surface/browser checks pass, including refresh, re-click, duplicate click, and narrow/error states                                                                                                                                                                                 |
| `D-04` | D     | One hermetic Launch-to-PR happy path                                                                                                                                                        | Exactly one Launch, fork intent/result, Launch branch, workspace, sandbox, session, owner initial message, contribution commit path, and pull request are observed; retries do not increment counts                                                                                                       |
| `D-05` | D     | Complete external-write and lifecycle fault matrix from section 5                                                                                                                           | Every row has a deterministic test anchor and pass result; unknown outcomes reconcile rather than blindly write                                                                                                                                                                                           |
| `D-06` | D     | Repository fixture matrix from section 6                                                                                                                                                    | Every fixture is bounded, synthetic, reproducible, and covers its expected validation/publication result                                                                                                                                                                                                  |
| `D-07` | D     | Credential/content redaction and wrong-authority tests                                                                                                                                      | Planted markers never enter browser, prompt, sandbox, logs, traces, activity, support, or artifacts; wrong owner/repository/ref/session is denied before effect                                                                                                                                           |
| `D-08` | D     | Deterministic rollback and recovery simulation                                                                                                                                              | Pause, kill switch, takeover, read-only reconciliation, cancellation, and cleanup preserve session and GitHub resources and retain exact resource counts                                                                                                                                                  |
| `D-09` | D     | Pinned Trigger runtime network-boundary campaign                                                                                                                                            | The exact release image proves the listener inventory, Codex/ACP/system-bubblewrap identity and reviewed artifact hashes, exact permission profile, mandatory negative probes, signed numeric-resource gateway call, traversal refusal without an outbound call, and raw-`CONNECT` denial described below |
| `P-01` | P     | Disposable public-repository end-to-end smoke                                                                                                                                               | Anonymous click through explicit PR produces the exact counts in section 7, validates real GitHub/NyxID identifiers and attribution, and proves the dedicated public observer forwards no subject, UserService, bearer, or provider credential                                                            |
| `P-02` | P/S   | Issue #131 real session-side GitHub evidence                                                                                                                                                | An `autonomous` scoped session uses the broker without manual approval, denies ungranted reach, survives replacement with its turn/skills, and fails closed without a profile                                                                                                                             |
| `S-01` | S     | Issue #675 restart and authority campaign                                                                                                                                                   | Fork-pending restart, running-session restart, access expiry with and without refresh token, inactive/reconnect service, and authority drift have redacted passing evidence                                                                                                                               |
| `S-02` | S     | Issues #445/#524/#528 credential campaign                                                                                                                                                   | Provider expiry is read back and enforced, exact-runtime replacement acknowledgement is proven, predecessor remains until promotion, duplicate reconciliation converges, and revocation is acknowledged or honestly TTL-bounded                                                                           |
| `P-03` | P/S   | Real-provider refusal and ambiguity campaign                                                                                                                                                | Rate limit, outage, 403/422, timeout-after-success, stale head, disabled forking, quota, occupied fork network, renamed/reclaimed repository, and workflow-scope refusal produce the typed results from section 5 without duplicates                                                                      |
| `O-01` | S     | Operator recovery drill                                                                                                                                                                     | The drill in the operations runbook is executed against the final build, reviewed, and linked; a written procedure alone is not a pass                                                                                                                                                                    |
| `O-02` | S     | Dashboard and alert wiring                                                                                                                                                                  | Saved dashboard and every alert ID in the operations runbook have deployment-specific links, an owner, a tested notification target, and a matching evidence interval                                                                                                                                     |
| `M-01` | M     | Security review                                                                                                                                                                             | Identity, no-key README, OAuth custody, inert runtime credential, signed Layer 4 policy, bounded revalidation, at-most-once dispatch, artifact download, cleanup, and revocation wording are approved                                                                                                     |
| `M-02` | M     | Product/repository go/no-go                                                                                                                                                                 | #470/rebrand and repository licensing posture have explicit maintainer decisions; neither is inferred from implementation                                                                                                                                                                                 |
| `M-03` | M     | Final release decision                                                                                                                                                                      | The accountable maintainer reviews this complete ledger, residual incidents and beta feedback, and records go/no-go for public exposure                                                                                                                                                                   |

The generic platform release condition from #484 is already satisfied and its
rate/request-size limits are consumed here. Issue #671 remains the post-MVP
sponsored-credential boundary and does not gate the contributor-owned public
MVP.

### Pinned runtime network evidence

`D-09` must run inside the Linux/amd64 image built from the release commit, not
in a host substitute and not against a floating Codex installation. The image's
`org.opencontainers.image.revision` label must equal the full workflow commit;
a caller-supplied commit string is not image identity. The record adds the
pinned Codex and ACP versions, `/usr/bin/bwrap` path and version, the reviewed
ACP, Codex launcher, native amd64 Codex executable, system bubblewrap, and
runtime hashes, the immutable image digest, and the image's IPv4 listener
inventory to section 2's fields. It must prove all of the following in one
reviewable run:

- the supervisor listens on a discovered non-loopback pod address at `9090`,
  the model proxy on `127.0.0.1:9091`, the two closed denial fronts on
  `127.0.0.2:9090` and `127.0.0.2:9091`, and the signed gateway on
  `127.0.0.2:9092`, with no `0.0.0.0:9090` listener;
- the actual Trigger permission profile admits only `127.0.0.2`, both denial
  fronts return a body-free response with only the exact isolation marker and
  safe transport headers, and `127.0.0.1`, `localhost`, a public destination,
  and the supervisor's non-loopback address return the managed proxy's closed
  `blocked-by-allowlist` denial;
- a valid probe reaches only the signed catalog-ID numeric-resource route with
  `_nyxid_via` bound to the exact UserService ID, while missing policy, wrong
  catalog/UserService/per-user-slug/resource/generation, provider refusal, and
  broker transience return their closed outcomes without retaining bodies;
- literal, encoded, double-encoded, mixed-dot, encoded-slash, and backslash
  path traversal use a valid HTTP authority and are rejected while a real
  sentinel credential is installed, with the fixture recording no outbound
  call. The same raw sandboxed campaign rejects `/proxy/s/` routes using
  `api-github`, a suffixed GitHub slug, a PAT-looking slug, the exact UserService
  ID, or the catalog service ID. This image evidence does not claim to observe
  whether an in-memory credential lookup occurred; and
- HTTP `CONNECT`, including authority-form requests executed through the actual
  `cma-trigger` Codex permission sandbox as well as directly against each
  listener, cannot tunnel to the real supervisor, model proxy, platform
  upstream, or any public destination; and
- after a verified workspace restore, a terminal-owned writer double-forks,
  changes sessions, and exits its ancestors; the supervisor adopts it, a
  hold-for-teardown checkpoint reports at least two quiesced writers, its file
  bytes remain stable while held, explicit resume restarts writes, and exit is
  reaped without a retained zombie.

The dedicated Trigger adapter sets `CHRONO_RUNTIME_WORKSPACE` to
`/home/chrono/workspace/repository`. A provider volume or the D-09 `tmpfs`
backs `/home/chrono/workspace`, its parent. The repository target must remain a
replaceable child on that same filesystem so verified staging can be committed
with one atomic rename; mounting the repository target itself would make that
rename fail with `EBUSY` and is not an admissible template.

Startup tests separately occupy each reserved listener and remove required
ingress, upstream, gateway-command, or policy-keyring configuration. Every case
must exit with code `2`, without a signal, before `/readyz` returns any HTTP
response on either pod or loopback ingress, and must emit its case-specific
safe refusal marker. A passing host-only probe, command-sandbox initialization
failure, or run against a different runtime artifact is inconclusive rather
than pass. A boundary response or startup refusal without its exact closed
marker is a failure.

The executable campaign is
[`cma-trigger-runtime-image-smoke.mjs`](../.github/scripts/cma-trigger-runtime-image-smoke.mjs).
Its CI and release jobs are pinned to Ubuntu 24.04 and follow the
[official Codex sandbox setup](https://developers.openai.com/codex/sandboxing):
install `apparmor-profiles` and `apparmor-utils`, copy the distribution's
targeted `bwrap-userns-restrict` AppArmor policy into `/etc/apparmor.d`, and load
it with `apparmor_parser -r` before execution. The system `/usr/bin/bwrap` binary
remains part of the image identity; the host policy is test-harness
provisioning. Only the campaign runtime receives `seccomp=unconfined`,
`apparmor=unconfined`, and `systempaths=unconfined`; the inner Codex permission
sandbox remains mandatory. A profile-load failure or sandbox bootstrap refusal
closes the run as non-passing; it never authorizes a global AppArmor change,
added capability, privileged container, host network, or unsandboxed substitute.

The pull-request runtime matrix runs it against the exact `cma-runtime:ci`
image loaded for the vulnerability scan and always retains its closed
`pass`, `fail`, or `inconclusive` JSON record as the 90-day
`cma-trigger-d09-*` workflow artifact. A `pass` also requires verified removal
of every labelled campaign container and network. This pull-request result is
an early deterministic gate, not the final release-image claim: the final run
must pull and test the immutable `sourceReference` digest validated by the
release workflow's OCI export, and GitHub Release creation must depend on that
run. The version and commit image tags necessarily publish first so there is an
immutable digest to pull; D-09 does not gate that initial GHCR tag publication.
The ledger remains `open` until that final-candidate workflow URL and artifact
are linked and reviewed; a local run is diagnostic evidence only.

## 4. Disposable public fixture

The public fixture contains synthetic content only and must never be
`ChronoAIProject/cma`, which is internal and fork-disabled. The selected fixture
is [`ChronoAIProject/cma-trigger-fixture`](https://github.com/ChronoAIProject/cma-trigger-fixture).
Its policy, source commit, and tag were read back from GitHub on 2026-08-28; this
selects the repository but does not substitute for disposable contributor
credentials or a passing provider run.

The fixture record must be committed or linked before `P-01` runs:

| Field                  | Required value                                                                                                                                                                                                                                                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Steward                | `chronoai-shining`                                                                                                                                                                                                                                                                                                                    |
| Repository             | `https://github.com/ChronoAIProject/cma-trigger-fixture`; repository ID `1349237845`; node ID `R_kgDOUGu8VQ`; organization owner ID `253878174`; owner node ID `O_kgDODyHfng`                                                                                                                                                         |
| Visibility/fork policy | Public with `allow_forking=true`, positively observed on 2026-08-28; recheck immediately before each run                                                                                                                                                                                                                              |
| Contents               | `LICENSE`, `README.md`, `FIXTURE_POLICY.md`, `.gitignore`, `bin/fixture-check`, `docs/task.md`, and `src/message.txt`; the committed policy excludes production source, user data, external dependencies, secrets, workflows, webhooks, deploy keys, environment secrets, and execution of fixture content                            |
| Default/source refs    | Default branch `main`; immutable source commit `878dcb6288f94721de6724899ea0680e1086d511`; lightweight tag `fixture-v1` resolves to that exact commit                                                                                                                                                                                 |
| Accounts               | Open: dedicated disposable publisher and contributor identities with least privilege are still required; `chronoai-shining` is the fixture steward and is not evidence of that two-identity separation                                                                                                                                |
| NyxID service          | Open: a disposable canonical-catalog GitHub OAuth UserService for the disposable contributor is still required. The existing maintainer service proves catalog-ID route reachability only; its `repo,read:user,user:email` grant is intentionally refused as `trigger_scope_unapproved` and is not the least-privilege `P-01` credential                                                                 |
| NyxID public observer  | Open: a dedicated credential-free public service slug, bounded `GET /repositories/**` anonymous rule and daily quota are still required; evidence must prove identity propagation, access-token forwarding, delegation-token injection, and resale billing are disabled and that CMA narrows the request to one numeric repository ID |
| Retention              | Evidence metadata follows the security evidence policy; contributor-owned GitHub fixture resources remain until review, while session stop removes the live `BatchSandbox` and retained `before_launch`, `live_session`, and `after_stop` metadata records that reconciliation                                                        |
| Cleanup                | Trigger cleanup never deletes or resets a fork, branch, commit, or pull request and never enumerates or deletes a sandbox. The existing session lifecycle remains the only sandbox teardown authority. The fixture steward manually closes/archive-marks test PRs and archives a superseded fixture only after evidence review        |
| Rotation               | Create and validate a replacement fixture before archiving the old one; pin evidence to numeric identity and SHA so rename/reclaim cannot silently retarget it                                                                                                                                                                        |
| Quota                  | A pre-run inventory confirms fork-network, branch, PR, storage, sandbox, and API headroom; stop rather than deleting another run's resources                                                                                                                                                                                          |

The repository must not run contributed workflows with privileged secrets.
Archive attributes, LFS pointers, malicious paths, and prompt-injection text are
test data, not instructions to the operator or agent. Fixture changes receive
ordinary review and are pinned by SHA in every run.

## 5. External-write and lifecycle fault matrix

Every external effect is tested at four crash boundaries: before claim, after
durable claim but before any call, after bytes may have been written, and after
positive provider success but before CMA records completion. Timeout after
possible write must enter read-only reconciliation or `recovery_required`; only
positive proof of pre-write failure may return to automatic execution.

| Boundary             | Required injections                                                                                                                                                                                  | Required proof                                                                                                                                                                                                     |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Fork                 | timeout after success, 5xx, 403/422, rate limit, outage, disabled forking, quota, occupied fork network, rename/reclaim                                                                              | Numeric fork-network lookup reconciles one compatible contributor fork; CMA never deletes or repurposes an incompatible fork                                                                                       |
| Branch               | timeout after success, occupied name, stale/advanced ref, missing ref, repository rename/reclaim                                                                                                     | One Launch branch points at the captured source SHA; no force-push, reset, or write to a name with mismatched intent                                                                                               |
| Workspace/archive    | corrupt/truncated archive, hostile path, symlink escape, LFS pointer/object absence, archive attributes, tree truncation, a blob over 16 MiB, repository over 100,000 files or 2 GiB, storage outage | Import refuses an oversized tree/blob before blob allocation, otherwise fails closed or records the bounded representation; no path escapes, credential extraction, partial baseline claim, or unbounded tree walk |
| Sandbox/session      | capacity/quota, provider `503`, no credential, crash, runtime loss/reprovision, control-plane restart                                                                                                | At most one entitled sandbox/session identity; checkpoint recovery preserves history/workspace and no feature flag interrupts the session                                                                          |
| Capability           | NyxID outage, inactive service, revoked OAuth, expiry/refresh, replacement, authority drift, disconnected runtime, generation rollback                                                               | Exact service and numeric resource policy fail closed; predecessor/revocation semantics match #524/#528/#675/#676 without exposing key material                                                                    |
| Initial task         | pre-write failure, post-write timeout, crash in each delivery state, late evidence, owner-authorized replacement                                                                                     | One automatic owner turn; ambiguity is fenced; each explicit owner recovery authorizes exactly one new generation and discloses duplicate risk                                                                     |
| Checkpoint/diff      | dirty workspace, corrupt snapshot, rotation, runtime loss, artifact timeout, hostile download range/header                                                                                           | Complete sealed owner/Launch-bound artifact or typed failure; no partial publication, cross-owner read, path/body leak, or workspace loss                                                                          |
| Commit/publication   | Git Data 5xx/rate limit, stale head, binary/mode/symlink/delete, large change, author mismatch, workflow warning/refusal                                                                             | Exactly the reviewed digest is published once with exact author/committer identity; unexpected head and scope refusal do not force or broaden authority                                                            |
| Pull request         | timeout after success, 403/422, duplicate click, browser refresh, existing marked PR, open/update mismatch                                                                                           | Stable marker and numeric head/base identities reconcile one PR; Open never silently becomes Update and no automatic merge/review occurs                                                                           |
| Cancellation/rollout | cancel before/after every boundary, feature pause, capability kill switch, cleanup dry-run/partial failure                                                                                           | Settled work stops at a durable boundary; ambiguous effects stay fenced; session and all GitHub resources remain; cleanup names only exact CMA workspace objects                                                   |

Retry budgets, provider `Retry-After`, scheduler page/concurrency bounds, lease
takeover, cursor repetition, and page-budget exhaustion are part of `D-05`.
They are not waived because the effect-level test passes.

## 6. Repository fixture matrix

Fixtures may be generated during tests when committing them would create a
large repository. The generator, seed, byte/entry limits, expected manifest,
and expected outcome must be deterministic.

| Fixture                                | Required assertion                                                                                                                                                                                     |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| untracked, modified, and deleted paths | Full diff and commit tree preserve all three classes                                                                                                                                                   |
| binary and large files                 | Binary classification, 4 MiB staging chunks, the 16 MiB single-blob limit, 2 GiB repository bound, and artifact digest are stable; over-limit content produces the typed refusal before a blob request |
| executable mode changes                | Git mode is retained without executing fixture content                                                                                                                                                 |
| safe and escaping symlinks             | Safe link metadata is represented; absolute or escaping traversal is refused                                                                                                                           |
| malicious names                        | `..`, absolute paths, control characters, case/Unicode collisions, archive aliases, and reserved paths cannot escape or overwrite another entry                                                        |
| prompt injection                       | Repository text remains untrusted content and never changes policy, credential routing, setup, review, or confirmation behavior                                                                        |
| upstream divergence/stale fork         | Numeric identity and captured SHA remain authoritative; no silent rebase, reset, or force-push                                                                                                         |
| archive attributes                     | Attributes cannot invoke filters, commands, hooks, or smudge behavior during import/publication                                                                                                        |
| LFS                                    | Pointer and missing/oversized object outcomes are explicit; no hidden credentialed LFS fetch occurs                                                                                                    |
| truncated large tree                   | Page/entry/byte ceilings produce an explicit incomplete/refusal state, never a falsely complete diff                                                                                                   |
| workflow files                         | Paths are detected, displayed, acknowledged where permitted, and refused without the required scope                                                                                                    |

The deterministic anchor inventory for this matrix is:

| Fixture row                            | Required test anchors                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| untracked, modified, and deleted paths | `trigger::workspace_diff::tests::golden_diff_covers_every_supported_file_state_and_is_deterministic`, `trigger::github::tests::git_data_publication_persists_each_object_boundary_and_is_verified`, and `workspace::tests::capture_round_trip_preserves_the_complete_current_tree`                                                                                                                                                                                                                                                                                                                                         |
| binary and large files                 | `trigger::github::tests::workspace_import_preserves_binary_executable_and_symlink_identity`, `trigger::github::workspace::tests::source_tree_file_blob_and_repository_bounds_fail_before_blob_reads`, `trigger::github::tests::workspace_import_refuses_an_oversize_provider_blob_before_requesting_it`, `trigger::github_effects::tests::workspace_import_reuses_complete_blobs_and_converges_partial_chunks_after_restart`, and the golden diff test above prove binary identity, stable artifact digests, 4 MiB chunks, the inclusive 16 MiB and 2 GiB limits, the 100,000-entry ceiling, and refusal before blob reads |
| executable mode changes                | The workspace-import, golden-diff, Git Data publication, and runtime `workspace::tests::a_verified_archive_restores_binary_executable_and_symlink_entries` tests prove mode retention without execution                                                                                                                                                                                                                                                                                                                                                                                                                    |
| safe and escaping symlinks             | The workspace-import and runtime restore tests prove inert safe-link representation; `trigger::workspace_diff::tests::unsafe_paths_and_symlinks_are_rejected_before_artifact_generation` and `workspace::tests::traversal_and_git_control_paths_are_rejected_before_extraction` prove escaping refusal                                                                                                                                                                                                                                                                                                                     |
| malicious names                        | `trigger::github::workspace::tests::malicious_repository_names_are_rejected_before_blob_planning`, `trigger::workspace_diff::tests::unsafe_paths_and_symlinks_are_rejected_before_artifact_generation`, and `workspace::tests::malicious_archive_names_aliases_and_reserved_paths_never_materialize` cover traversal, absolute/control names, case and Unicode aliases, archive aliases, and reserved paths at independent boundaries                                                                                                                                                                                      |
| prompt injection                       | `trigger::github::workspace::tests::repository_content_and_non_lfs_attributes_remain_inert_archive_bytes`, `trigger::tests::hostile_repository_content_cannot_mutate_launch_policy_authority_or_confirmations`, and `workspace::tests::attributes_and_hostile_text_restore_as_inert_exact_bytes` prove repository text remains inert across import, policy, and runtime boundaries                                                                                                                                                                                                                                         |
| upstream divergence/stale fork         | `trigger::github::tests::compatible_fork_is_reused_only_after_network_and_commit_proof`, `trigger::github::tests::divergent_compatible_fork_is_reused_without_resetting_its_default_branch`, `trigger::github::workspace::tests::workspace_inventory_remains_bound_to_the_launch_source_when_a_branch_moves`, and `trigger::workspace_diff::tests::no_changes_and_stale_head_are_named_review_outcomes` preserve numeric identity and the captured SHA without reset, rebase, or force                                                                                                                                     |
| archive attributes                     | The repository-content and runtime exact-byte tests above cover `export-subst`, `export-ignore`, `eol`, filter, and executable attributes without invoking commands, filters, hooks, or smudge behavior                                                                                                                                                                                                                                                                                                                                                                                                                    |
| LFS                                    | `trigger::github::workspace::tests::lfs_attributes_and_pointer_files_are_both_refused`, `trigger::github::tests::workspace_import_refuses_lfs_attributes`, and `trigger::github::publication::tests::publication_refuses_new_lfs_attributes_and_pointer_content` prove typed refusal for imported and newly introduced LFS metadata or pointers without a hidden object fetch; the oversize-provider-blob test proves the independent object-size refusal before request                                                                                                                                                   |
| truncated large tree                   | `trigger::github::tests::workspace_import_refuses_truncated_tree_before_any_blob_read` and the source-tree boundary test above prove provider truncation and an over-ceiling inventory become explicit incomplete/refusal outcomes before blob reads                                                                                                                                                                                                                                                                                                                                                                       |
| workflow files                         | `trigger::workspace_diff::tests::workflow_scope_is_checked_at_review_with_same_content_exception`, `trigger::github::tests::provider_refusals_remain_named_after_ref_reconciliation`, and `trigger::tests::workflow_paths_require_an_explicit_reviewed_confirmation_before_start` cover detection, display/confirmation authority, same-content handling, and typed scope refusal                                                                                                                                                                                                                                          |

These anchors are class `D` evidence only. `D-06` remains open until the final
review revision links their passing CI results, and they do not establish real
GitHub/NyxID behavior.

## 7. Real-provider smoke contract

The real-provider smoke is separately gated from default CI. It runs only by an
explicit authorized dispatch against the selected disposable fixture and
disposable credentials. Pull requests from forks, ordinary pushes, scheduled
jobs, and untrusted input cannot obtain its environment or secrets. Its
concurrency key allows one fixture run at a time.

The executable entry point is
[cma-trigger-provider-smoke.yml](../.github/workflows/cma-trigger-provider-smoke.yml).
Its checked-in
[example configuration](../.github/cma-trigger-provider-smoke.example.json) is
schema and harness input only; its placeholder identities and invalid origin
are not provider evidence. The live `cma-trigger-smoke` environment, ID
`20761349259`, was read back on 2026-08-28 with administrator bypass disabled,
self-review prevented, the `AI Application` team (team ID `15748251`) required
to review, and deployment restricted to protected branches. Keep every smoke
variable and credential there rather than at repository scope. The plan job
reads that live environment configuration with the
workflow token's exact read-only `actions: read` permission and fails closed if
the endpoint is unavailable or any protection is absent. The token also retains
`contents: read` for checkout and has no write permission. Only then does the
environment supply the reviewed non-secret identities and disposable CMA
credential after an authorized operator dispatches the workflow from `develop`
and accepts the preservation rule. The provider job repeats the same live check
after environment approval and before checkout, dependency installation, or
credential-bearing execution, closing a protection-change race. The environment
still has no reviewed candidate provenance variables, live configuration,
reviewed Trigger sandbox-namespace binding, disposable CMA credential, scoped
smoke inventory credential, or completed independent approval. The provider
lane is also unprovisioned. The harness therefore exists, but `P-01` remains
unavailable and open.

The protected environment must bind the full deployed build commit,
`sha256:` runtime image digest, exact runtime version advertised by Trigger
admission, and immutable deployment configuration revision. These are reviewed
provenance inputs, not values typed ad hoc into a dispatch. The provider job
queries the workflow run's environment-approval history after the gate opens;
it records the numeric GitHub identities of the actual triggering operator and
at least one independent approving reviewer and fails closed if the approval
cannot be tied to `cma-trigger-smoke`. GitHub's review-history response does not
carry an approval-submission timestamp, so the record honestly names the UTC
time at which the workflow observed that approved history; it does not invent a
`reviewed_at` value. The opaque NyxID service value remains a secret and is
represented only by its SHA-256 fingerprint. The harness verifies
the configured runtime version against both review-time and Start-time
admission. It records the runtime image digest as approved deployment
provenance; `D-09`, not this workflow, proves the bytes and boundary behavior of
that digest.

The additional required protected-environment variables are
`CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA`,
`CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST`,
`CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION`, and
`CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION`, plus
`CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE`. The namespace must equal the deployed
`CHRONO_TRIGGER_SANDBOX_NAMESPACE` and is materialized as the closed fixture
field `trigger_sandbox_namespace`; it is a reviewed deployment binding, not an
ad hoc dispatch value. The protected secret
`CMA_TRIGGER_SMOKE_KUBECONFIG_B64` contains the base64-encoded credential for a
separate smoke observer. Its namespace-local, list-only Role grants `list` on
`batchsandboxes.sandbox.opensandbox.io`; it grants no other verb, no
cluster-wide list, and no listing of `secrets`, `configmaps`, or `pods`. This is
not CMA's production reclaimer Role, which legitimately retains namespace-local
`get`, `list`, and `delete`. The remaining fixture bindings are materialized
into the same closed configuration shown by the checked-in example. The
operator/reviewer values are workflow-derived outputs and must not be
maintainer-supplied variables.

Before running, the operator verifies the build and runtime digests, closed
rollout allowlists, fixture numeric identity/SHA, service/account identity,
fork-network inventory, quotas, and redaction. Production is not restarted or
deployed solely to collect the result.

The passing smoke records exactly:

```text
1 Launch
1 compatible contributor fork created or 1 explicitly recorded compatible reuse
1 Launch-owned branch
1 Launch workspace
0 BatchSandboxes before Launch (0 entitled, 0 unexpected)
1 live entitled BatchSandbox (1 total, 0 unexpected)
0 BatchSandboxes after session stop reconciliation (0 entitled, 0 unexpected)
1 CMA session
1 automatic initial owner message
1 reviewed contribution commit path
1 open or updated pull request matching the explicit operation
```

The record distinguishes created from safely reused resources. The assertion is
not weakened to "at least one." A rerun with the same idempotent intent must not
increase any count. A New Launch is a separate, visibly confirmed case and is
never used to make the smoke pass after failure. The three `BatchSandbox`
observations are complete, unpaginated lists of the exact Trigger namespace;
any continuation token fails the run. `counts.sandbox` is derived only from
`live_session.entitled_objects.length`, never inferred from Session state. The
post-stop observation polls until inventory is empty or fails with
`sandbox_leaked_after_stop`.

The retained JSON uses closed evidence schema version 3; the separate fixture
configuration remains schema version 2. In addition to the section 2 fields,
the evidence binds a deterministic evidence ID to the workflow run attempt, the
exact fixture-file SHA-256, the fixed no-fault `P-01` rollout action, safe
Launch and GitHub request correlations, typed resource identities, and exact
before/after/delta inventories for Launches, contributor forks, Launch branches,
and matching pull requests. Each `BatchSandbox` observation also retains only
allowlisted Kubernetes metadata: API version and kind, namespace and name, UID,
object and list `resourceVersion` values, creation and deletion timestamps, and
the values of `opensandbox.io/id`, `cma/kernel`, `chrono-bot/kernel`,
`cma/session`, `chrono-bot/session`, and `cma/runtime-generation`. The
`cma/runtime-reservation` label is validated but retained only as its SHA-256;
the raw reservation is never serialized. The artifact name and the run's
`#artifacts` landing URL are deterministic before upload; the workflow also
exposes the upload action's direct retained-artifact URL as a job output once
GitHub assigns the artifact ID. Neither URL is a claim that the artifact exists
until the upload step succeeds.

The workflow installs pinned `kubectl` v1.34.1, decodes the observer credential
with `umask 077` into
`${{ runner.temp }}/cma-trigger-smoke-kubeconfig`, and enforces mode `0600`.
Under `always()`, it removes that file before running the credential and
redaction audit, including when the smoke fails. Kubeconfig bytes, Kubernetes
bearer tokens, client key or certificate data, and raw runtime reservations
must never enter configuration, logs, or evidence. The workflow uploads the
manifest and result only after that audit passes and the terminal document
passes the closed publishable-schema validator. The audit atomically marks
redaction complete; a running, extended, mistyped, contradictory,
secret-bearing, or incomplete document is never published as an artifact.
Checked-in mutation tests reject extra, stale or ambiguous, missing-live, and
leaked-after-stop inventory; a raw reservation; a fabricated sandbox count;
and representative missing-field, provenance-drift, self-review, identity,
correlation, URL, and redaction failures. A structurally valid `fail`,
`inconclusive`, or `blocked` record may be retained; it never advances `P-01` to
pass. The workflow must not automatically delete contributor-owned GitHub
fixture resources after the run. Normal CMA session stop remains the sole
sandbox teardown authority and must reconcile the live `BatchSandbox` to the
required zero-object `after_stop` inventory.

## 8. Staged rollout and exit criteria

The exact controls and activation order are in
[CMA_Trigger_Operations.md](./CMA_Trigger_Operations.md#4-staged-activation).
Each stage changes one control at a time and records its configuration revision.

| Stage               | Exposure                                                                                           | Entry gate                                                                       | Exit gate                                                                                                                                                                                                       |
| ------------------- | -------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0. Dormant deploy   | No publisher or Launch admission; orchestration paused; capability kill switch on; cleanup dry-run | Migrations/indexes reviewed and rollback-capable image retained                  | All scheduler lanes healthy, no prerequisite gap, no existing-session change, `D-01` through `D-09` pass on the deployed commit and exact pinned runtime image                                                  |
| 1. Staging provider | One synthetic publisher, contributor, and fixture; no production subjects                          | `P-01` configuration reviewed and disposable credential custody confirmed        | `P-01`, `P-03`, `S-01`, and `S-02` pass; all planted leakage scans are zero                                                                                                                                     |
| 2. Internal canary  | One repository allowlist and named internal subjects; cleanup remains dry-run                      | Security review has no open SEV-0/SEV-1 finding; alerts route to a tested target | At least 10 completed Launches across two contributor accounts, every required fault/recovery drill passes, and no unexplained resource-count growth occurs                                                     |
| 3. Invite beta      | Explicit publishers and a stable small subject cohort                                              | `O-01`, `O-02`, `M-01`, and publisher documentation pass review                  | At least 50 started Launches from five contributors over seven consecutive days; 99% of non-user-blocked steps settle/defer/type-block within 10 minutes; no SEV-0/SEV-1 incident or credential leak            |
| 4. Limited public   | Public README links only for allowlisted repositories; bounded cohort                              | `M-02`, all preceding ledger rows, and beta feedback triage are complete         | At least 100 Launches over 14 consecutive days, zero duplicate automatic dispatch or destructive cleanup, no unresolved reconciliation debt past its SLO, and rollback drill replay passes on the release build |
| 5. Widened public   | Increase repository/publisher allowlists or subject cohort one step at a time                      | `M-03` go decision and a named watch owner for the change window                 | One full alert/SLO observation window passes before another widening; any stop condition returns to the prior stage                                                                                             |

User-action blocks and provider-caused waits are reported separately from CMA
availability. They remain visible in beta feedback and cannot be discarded to
improve a success rate. A sample target is a minimum, not permission to ignore a
failure found before the target is reached.

Stop widening and apply the runbook rollback sequence on any credential/content
leak; session interruption; duplicate automatic task; unauthorized repository,
ref, or operation; cleanup outside exact CMA workspace targets; automatic
force-push/merge; stale-policy acceptance beyond its bound; unexplained external
resource increase; or missing audit/reconciliation evidence.

## 9. Beta feedback

The beta owner is `chronoai-shining`. Feedback is triaged at least once per
business day during stages 3 and 4 and before every widening decision.

The intake record contains only:

```text
feedback ID and UTC time
rollout stage and build/configuration revision
safe Launch and correlation IDs
publisher or contributor role (not a copied credential identity)
category: discovery, consent, review, progress, session, diff, publication,
          recovery, performance, accessibility, or documentation
expected behavior, observed safe summary, severity, and reproducibility
linked telemetry interval and linked issue
```

Do not ask a beta user to paste tokens, authorization headers, prompts,
repository content or paths, sandbox files, provider bodies, or diff artifacts.
Support uses the redacted Launch/support projection. Security, cross-owner,
duplicate-effect, session-continuity, and deletion reports bypass ordinary
product triage and enter the runbook severity path immediately.

Before stage exit, every SEV-0/SEV-1 report is resolved and reverified, every
SEV-2 report has an owner and rollout decision, and repeated usability failures
have a linked product issue. Report counts by category and resolution without
publishing user content.

## 10. Post-MVP backlog

These items require their own reviewed policy or design. They are not partially
enabled by an MVP feature flag:

- #671 publisher-sponsored credentials, grants, billing, spend, abuse controls,
  revocation, and disclosure;
- private/internal repositories and organization installation approval;
- publisher-authored setup commands and a reviewed executor policy;
- broader workflow-file OAuth consent and enablement beyond the MVP's exact
  warning, scope preflight, same-path/same-content exception, and typed refusal;
- richer Git operations such as synchronization, rebase, conflict resolution,
  multiple branches, force operations, or Git smart HTTP;
- #705 wire-level origin/turn identifiers for recipient-side prompt
  deduplication beyond MVP at-most-once dispatch;
- GitHub App installation credentials under a replacement credential ADR;
- maintainer-side review, check, merge, and repository integration workflows;
- custom organization-role authorization after real provider evidence; and
- expanded fixture automation that can prove lifecycle safety without deleting
  contributor-owned GitHub resources.

Automatic merge, branch-protection bypass, upstream writes, and hidden consent
are not backlog promises; they remain explicit non-goals unless a later product
and security decision changes the boundary.
