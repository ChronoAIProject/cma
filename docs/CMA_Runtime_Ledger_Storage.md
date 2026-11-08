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
