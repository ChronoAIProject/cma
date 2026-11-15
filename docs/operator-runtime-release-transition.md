# Fenced operator runtime release transition — implementation draft

This backup implementation is **NOT RUNNABLE AS ROLLBACK**. It cannot yet
re-promote a prior runtime release. Do not enable managed mode or treat a
passing component test as permission to migrate, switch releases, or deploy.
Issue #2097 tracks completion and the required real replicas-OFF qualification.

An ordinary API rollback cannot re-promote a superseded runtime image: pool
activation dates are immutable, and a new plan for an old image retains its
original release date. The proposed supported operation selects an atomic
agent/machine release pair with a monotonic deployment epoch, preserving the
original pool/template/home/Claim identities and activation history.

## Implemented source boundaries

- A transactional release head, immutable operation history, permanent effect
  permits, same-ID replay, and unresolved same-request deduplication. Only a
  newly acknowledged permit allows dispatch. An uncertain transaction outcome
  never allows automatic replay.
- Managed native target polling uses a closed Mongo transaction for head admission,
  original target lease, fixed-lane cursor and immutable result. Empty results
  do not leave provider permits behind. Cancellation retains the same command;
  historical Empty does not rescan, and positive Replay never dispatches.
  Another poll on that lane waits for the exact target/revision/lease finish CAS.
  This acknowledges only the store operation, without settling provider effects.
- Freeze and effect admission update the same head in their transactions.
  Provider ACKs append observations; only verified terminal evidence may settle
  an effect. Cancellation, process exit and elapsed deadlines are not terminal
  evidence. Repeated Hold does not consume revision/history; a separate terminal
  receipt preserves recovery when an older history already contains 32 entries.
- Explicit inert startup before ordinary store/profile/provider/worker
  composition. Each process publishes a new private challenge and requires an
  expiring Ed25519 grant for that exact instance, input/effective-key fingerprint,
  executable digest and configured immutable images. The grant is inert-only.
- Personal authenticated finite reads for `GET /api/v1/sandboxes`,
  `GET /api/v1/sandboxes/{id}` and
  `GET /api/v1/sandboxes/{id}/workspaces`. These use restricted read stores and
  existing Layer1 views/Layer2 hierarchy ownership checks. Existing browser
  credentials are verified against the identity provider and durable login,
  token generation and expiry before/after reads. Expired or renewal-dependent
  credentials, organization/shared requests and mutations refuse. This does
  not provide complete web-app or stream continuity.
- Original selected pool and template UIDs, exact plans, target revisions and
  positive observed readiness are checked without repairing or resizing the
  destination. Current and fallback releases remain protected from retirement.
  Managed mode retains provider target-revision fencing with replicas OFF.
- Provider routing, escaped native-pool/home capabilities and native target
  mutations acquire permits. A process-lifetime obligation remains unresolved
  across composition and detached recovery. The HTTP maintenance check is
  read-only, after authentication/rate admission; it neither authorizes a later
  domain write nor changes Layer1 idempotency semantics.
- A private-file operator interface. Fresh existing-session `ConfigurePlatform`
  authority and an exact signed control request are required. Only Begin and
  Hold can currently obtain production-verifier evidence. Every action that
  bootstraps, selects, activates, reopens or settles authority refuses because
  its live finality collector is not implemented.

## Incomplete mandatory work

These are implementation dependencies, not optional operating caveats:

1. Acquire truthful legacy process/restart-source closure and complete
   provider, native-controller, receiver and retry finality. An empty new
   permit set, receiver ACK, Pod disappearance, CRI absence or quiet inventory
   alone cannot prove this. Receiver proof must include the exact container
   and process incarnation, terminal durable adoption and harmless duplicate
   delivery, or positive receiver/retry fencing plus provider finality.
2. Complete atomic logical workflow and durable-store admission, retaining
   exact operation identities across epochs. A request can pass the advisory
   HTTP check before freeze and persist later. Process lifetime prevents a
   false drain while the process lives; it does not prove queued work terminal
   after exit. Native polling now has a store-only result, but other target
   writes and downstream logical workflows still require complete admission,
   settlement and recovery. Unknown positive poll replay stays held; elapsed
   lease time never permits another lane to acquire that unresolved target.
3. Implement a guarded migration executor, source-bound per-row CAS, collection
   validators/indexes and authoritative completion receipts. Current code only
   emits a plan and inspects the installed strict schema3/4 floor. It never
   edits pool activation timestamps, revives tombstones or creates destination
   capacity. Startup refuses partial migration even when the head is absent.
4. Complete the signer/observer trust chain. A configured image string is not
   an observed container image. Before signing, the trusted operator boundary
   must independently verify actual source, image, Pod/container/process,
   effective configuration and retained home/Claim custody. A public key,
   signature or supplied checksum is not finality evidence.
5. Finish exact effect-to-domain/provider/receiver reconciliation. Persisted
   digests and retained unknown outcomes are admission evidence; a successful
   method return does not establish safe settlement or usable retry behavior.
6. Qualify the complete candidate against a real legacy-mode AM and native
   controller: agent and machine warm-Claim and cold paths; public create,
   pause/revive/remove; public readiness/authenticated RuntimeStatus/durable
   Active; retained home/PVC/Claim lineage; negative stale-writer and unknown
   outcome cases; authenticated read continuity and exact cleanup/finality.
   The current incident's cold-only, zero-warm-target fixture is not warm-path
   qualification. No production execution is authorized by this document.

## Configuration and authority

`infra/api/configmap.yaml` keeps `CHRONO_RUNTIME_RELEASE_MODE=disabled`.
Managed mode additionally requires `CHRONO_RUNTIME_RELEASE_PUBLIC_KEY`,
`CHRONO_RUNTIME_RELEASE_GRANT_PATH`,
`CHRONO_RUNTIME_RELEASE_CHALLENGE_PATH` and
`CHRONO_RUNTIME_RELEASE_BINARY_IMAGE`. No signing secret, operator cookie or
usable grant is supplied in the repository. Grant/request/receipt files must
be bounded private regular files. Receipts use a new path and are never
silently replaced.

An uninstalled deployment remains compatible with its ordinary Mongo role.
The optional managed role adds the four authority collections as a complete
set with only `find`, `insert`, and `update`: it does not grant remove, DDL or
foreign-service access. Partial and destructive isolated grants refuse.
Managed startup requires the complete role; migration authority remains a
separate, unimplemented executor. The existing configured isolated/shared
schema authority is also verified before inert reads.

The release head requires schema2. An installed head1 requires an explicit
quiescent migration and cannot start a writer through automatic compatibility
fallback. Existing operation/effect rows remain schema1. The fourth collection,
`native_runtime_store_commands`, stores schema1 cursors and immutable poll
receipts. It has no TTL or receipt deletion; polling volume and retained storage
must be qualified. The existing default-disabled installation check also
recognizes this collection before any ordinary composition effects.

The explicit interface is `cma --runtime-release REQUEST_FILE RECEIPT_FILE`.
The `migration_plan` request returns planning metadata and a role template,
with `apply_authority=false` and no database connection. Status, operation and
effect commands inspect existing authority records. `native_poll_tip` reads
the latest fixed-lane receipt for an exact writer. It grants neither dispatch
nor permission to replace a pending command. Control requests remain
subject to fresh operator authorization and exact state binding. A receipt
file or command exit is never a substitute for inspecting the operation state.

## Validation attribution

The original provider component at commit `f167641d4f0e3fdb42d3a6998ddc236bd693dc90`
passed six native tests and strict adapter Clippy. It does not cover subsequent
integration. The initial `e8dcb9d3f4b6e769483da6fe4cf208c1629ff905` integration
packet was prepared before the target-fencing, HTTP, role and fresh-auth
corrections and must not qualify later bytes.

Current candidate validation must bind the exact source/tree, runner, discovery
counts and process finality. Native component tests, real Mongo transactions,
real AM assignment, image qualification and production acceptance are distinct
results. The PR remains draft until its complete implementation and required
qualification have actually passed.
