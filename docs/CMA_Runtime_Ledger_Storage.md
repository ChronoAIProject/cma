# Runtime mutation ledger storage

The indexed V2 store is selected automatically only for a new retained home
with no existing home binding, installation state or mutation history. The
runtime requires an explicitly assigned retained transfer directory on the
same storage device as the home and state roots. Native provider deployments
mount these three subpaths from the same claim. Runtimes without a persistent home identity continue using V1. A new retained
home without the supported transfer layout holds instead of initializing V2. Previously initialized homes continue using
V1 even when the active generation has no process records. A restart does not
migrate an existing or exhausted ledger.

## Capacity and durability

Fresh V2 homes use a 1 GiB SQLite page ceiling and a separate, physically
allocated 16 MiB emergency reserve. Outcome and cleanup reservations share the
page budget. New work can reach the byte ceiling before the 1,024-outstanding
operation ceiling. Terminal history remains retained and eventually consumes
space; indexing removes the V1 terminal-record scan ceiling, not physical
storage limits. Receipt garbage collection is unsupported.

Each connection verifies DELETE rollback journaling and synchronous=EXTRA.
The storage provider must honor file and directory synchronization. Passing
local process-crash and injected-I/O tests does not qualify a production
storage driver for power-loss durability. Such qualification belongs to the
consented runtime release.

Storage errors keep outcomes ambiguous and refuse further normal admission.
The emergency reserve is released after a storage hold to leave room for
inspection and covered cleanup. It is not refilled automatically. Do not
truncate the database, delete receipts or tombstones, refill the reserve, or
increase configured limits to bypass a hold. Preserve evidence and use a
reviewed maintenance procedure.

## Format and rollback

The runtime stages and closes the new database, persists refusal descriptors
in the installation directory and both persistent/current-process V1 ledger
directories, then renames the database and publishes its current-format
manifest. The manifest is the only cutover commit point. A stable lifetime
writer lock lives outside the database directory. Interrupted publication,
missing descriptors, a replaced database, or conflicting identities hold;
there is no fallback to legacy files.

The installation descriptor is deliberately unreadable by V1 runtimes. An
accidental downgrade refuses before installation recovery, desktop or Agent
launch and authenticated control serving. The private assignment socket and
pre-install ownership metadata can still be created. Keep the retained
transfer mount unchanged across replacement: an ephemeral or different
installation root cannot provide this downgrade protection.

Before activation, qualify the refusal boundary against every exact older image
still selectable for that home, or enforce its exclusion. A source-reader
fixture or one prior-image result does not qualify other published binaries.
CI separately retains the executable's SQLite source ID, compile options,
create/commit/reopen result and ELF interpreter/dependency checks for each exact
default, Debian desktop and Omarchy image. These checks do not replace native
storage-driver durability or retained-home rollback qualification.

Roll back a V2 home only to an independently tested V2-capable runtime image
that retains every later fence, tombstone and receipt. Never remove format
barriers or restore a pre-cutover snapshot to start an old image. Old images
remain usable only with untouched V1 homes, where their capacity limitations
remain. Publishing a runtime image does not change production pins or admit
managed replacements.

## Legacy imports

No serving path releases an imported ledger. Imported provenance is durably
held before generation advancement, mutation recovery or lifecycle resumption.
This includes pending RemoveHome and RemoveWorktree operations under every
parent, including an Active parent. Owner disposition and preservation of
wanted files and history must be agreed and verified before any future
maintenance release. The maintenance-admission and release-authority protocol
is separate work tracked by #1461; this format does not provide an operator
release switch or authorize interruption of an existing home.

`cma-runtime --inspect-ledger-v1` reads the assigned runtime configuration from
the environment and emits only bounded counts and an inventory digest. Run it
only through a reviewed offline/read-only procedure; it does not start the
supervisor, change generations, recover temporary files or run pending work.
It includes persistent and historical process-scoped mutation files, workspace
journal files, installation state and home/removal-writer metadata. It refuses
V2 state and the separate workspace-repair authority layouts rather than
silently omitting them. File names and payload contents are not printed.

The structural inventory has separate limits of 65,536 examined directory
entries, 2 GiB input, 24 MiB metadata accounting and five minutes; no online V1
scan limit changes. Its summary explicitly reports semantic validation pending
and writer exclusion unproved. A stable hash is not proof of an atomic snapshot
or an absent legacy writer, and this command does not authorize migration or
release. The new V2 lock cannot fence a V1 supervisor that never takes it.

`cma-runtime --validate-ledger-v1` additionally validates every mutation record
before selecting authority survivors or interpreting terminal replacements.
It preserves the source bytes, including unpublished temporary files, and
does not qualify auxiliary workspace/installation recovery or writer exclusion.

`cma-runtime --plan-ledger-v1 /absolute/new/private-directory` makes a separate
byte-for-byte ledger backup and source manifest after that validation. The
destination must not already exist or overlap the home, state or transfer
roots; symlinked ancestors are refused. The directory is mode 0700 and completed
files are mode 0400. The private manifest contains source identities and paths;
raw records may contain receipt data. Keep the whole artifact in protected
storage. Only counts, fixed hold reasons and digests appear in command output.

The planner copies and syncs each source file, writes and syncs `plan.json`,
rescans the original input, then publishes `sealed.json` with the plan digest.
Any failed step leaves an incomplete artifact for inspection; a retry requires
a new destination and never overwrites a prior plan. A missing, malformed or
mismatched seal is not a completed backup. Preserve the reported plan digest
independently of the artifact for later exact-plan verification. These files
are a ledger backup, not a full home/history backup or permission to delete
anything. Even a sealed plan records writer exclusion as unproved: it supplies
no cutover, release or recovery authority and changes no source-root format.

`cma-runtime --verify-ledger-plan /absolute/private-directory <plan-sha256>`
reopens that exact artifact using the independently retained digest and the
original assigned runtime configuration. It reads only the archived ledger
bytes, so the original source can remain offline. A different source identity
or generation, changed record, missing/extra file, writable or linked copy,
or invalid seal holds. The manifest is streamed with a 24 MiB input ceiling
and bounded metadata collections. Verification does not establish that the
live source still matches the plan or that an old writer has been excluded.

`cma-runtime --stage-ledger-v1 /absolute/private-plan <plan-sha256>
/absolute/new/private-import` creates a detached SQLite artifact from the
verified copies. It uses the original assigned configuration, preserves exact
selected record bytes, and initializes `import_held` in the schema transaction.
It never writes the source home, publishes its format manifest or executes
recovery. The destination must be new and separate from both the plan and all
source roots. The planner's private byte backup remains required alongside it.

Staging imports one bounded authority group or operation per transaction,
derives outcome/cleanup reservations and process projections, verifies the
schema, counters, capacity and raw record round trips, then closes and syncs
the database. Only after revalidating the complete backup does it publish
`ready.json`, binding the plan digest and closed database digest. A missing
ready descriptor, changed backup, insufficient reservation budget or spent
emergency reserve holds. Preserve the failure artifact for inspection; retry
into a new destination, never overwrite or continue an ambiguous partial DB.
An artifact that is ready is still **held**, including all Started and pending
destructive work. Do not copy it over a live store or manufacture source-root
format markers. Source-root cutover requires the separately reviewed old-writer
exclusion and maintenance authority; a detached artifact supplies neither.

## Read-only diagnostics

An authenticated RuntimeStatus request may opt in with the exact Accept value
`application/vnd.cma.runtime-ledger-status.v1+json`. The response uses that media
type and a `{runtime, ledger}` envelope. The runtime object provides the usual
exact boot/home/generation attribution; ledger counters contain no identities
or receipt contents. Requests without this opt-in keep the existing JSON.
A retained runtime returns the baseline status frame, which the shared decoder
reports as unsupported, never as zero counts.

V1 observation reports `legacy_migration_required` with unknown counts and does
not scan or reconcile its journals. V2 observation reads a single committed
transaction revision. History totals cover all generations; normal-outstanding and
logical-process counts cover the persistent and current process scopes. Page
usage, reserved outcomes/cleanup bytes, shared transaction headroom and remaining
emergency reserve are reported separately. These reads do not wait for the
supervisor's external-effect gate or trigger recovery. Covered cleanup does
not consume normal-outstanding slots; zero normal-outstanding is not evidence
of quiescence or permission to release a maintenance hold. Worker saturation or
invalid storage returns an explicit hold instead of invented counters.

Treat 70% and 85% of the page or outstanding budget as warning thresholds and
any spent emergency reserve or capacity refusal as requiring investigation.
Headroom and reserved bytes must be included in byte-budget assessment. A
snapshot does not promise that the next operation will fit: admission checks
its actual outcome and cleanup reservations. Aggregate metrics must not use
Sandbox, Agent, Workspace or owner identities as labels.
