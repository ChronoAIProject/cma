# Recover interrupted initial runtime ledger publication

The runtime supports two bounded recovery paths before installation or effect
admission. Intent-bearing publications complete their exact existing store.
Recognizable generation-one staging prefixes without an intent are discarded and
initialized again. Neither path relaxes retained committed-store verification.
The incident's generic format-hold error does not prove which prefix was present;
root, device, ownership, mode and durable-I/O defects retain their own refusals.

The writer records `.mutation-ledger-v2.fresh.json` after closing and syncing its
new staged database, before publishing format barriers. The intent contains the
exact V2 descriptor. It never authorizes installing a different store or identity.
The manifest remains the format commit point. The writer must durably remove the
intent before returning a ledger to Supervisor startup or admitting any effects.

On an intent-bearing restart, the construction writer and outer ledger writer
remain locked. Inspection requires the same Sandbox, home, roots, generation and
store; a valid current home binding; no retirement state; only recognized initial
publication artifacts; and matching bytes for every present marker/descriptor.
There must be exactly one staged or final database. A committed manifest requires
the final database and every original barrier, with no stage or temporary manifest.
Only missing initial barriers may be completed; partially published generation
advancement is not recoverable through this path.

Database inspection pins the directory/database/writer inode tuple, acquires its
existing inner writer lock and uses an actual
read-only SQLite connection with no journal recovery, immutable view, initializer,
migration or generation normalization. The closed file must have no rollback/WAL
artifacts, retain the full physical reserve, and fit the explicit 512 KiB ceiling
for a fresh empty schema. Schema definitions and indexes must match exactly.
Metadata and accounting must each contain their exact singleton; generation is
equal, not merely less than the assignment. Every accounting field must be zero.
`authority`, `scope_budget`, `cleanup_budget`, `operations` and `logical_processes`
must all be empty. Missing tables, extra schema objects and malformed values are
refusals. The existing private-file, owner, link, ancestor and same-device checks
remain mandatory.

Recovery rechecks the pinned tuple, intent inode/content and authority before each
publication step and intent consumption, including after the database rename.
A path substitution must hold without modifying the replacement. Recovery
validates all existing state before creating a missing barrier. It syncs
the closed database, completes exact markers, renames the original database if
needed, commits the manifest, and durably consumes the intent. It never deletes
unknown data or chooses V1 because the manifest is absent. A second interruption
leaves either the same proof-bearing publication or an ordinary committed store.

A legacy no-intent prefix is eligible only while the final database path and
temporary/canonical manifest are absent. It requires the existing construction
and outer ledger locks, the exact un-restored generation-one home binding, only
recognized initial paths, no installation/archive/retirement state and no unknown
or advanced-generation authority. A nonempty staged DB must pass the same exact
read-only freshness proof above, and any existing barriers must match its store.
A pre-schema prefix is limited to an empty directory, zero-length DB and writer,
and bounded reserve bytes consisting solely of the allocator's `0xa5` pattern.
Malformed or nonempty unexplained database/journal bytes remain held.

Cleanup removes matching barriers first, then the proven unused database, reserve,
inner lock and staging directory. Every removal syncs its parent, rechecks the
complete path/inode inventory and keeps the outer lock inode held. If interrupted,
the remaining files are another recognized staging prefix. A replacement store
receives a new identity; a second ordinary restart retains that committed identity.
No durable intent is discarded. No committed manifest, final database, installation,
effects or retirement state enters cleanup. Final-DB-only and final-DB-plus-temporary
states remain held, even if their database is empty: a missing manifest is not
proof that a previously committed store was never served. Existing legacy PVCs
outside the eligible staging prefixes still require separate diagnosis.

“Committed assignment” here means the durable runtime publication/effect boundary,
not the in-memory bootstrap Accepted reply or the earlier AM native Claim. The
receiver can acknowledge assignment before Supervisor begins publication. The
format manifest is the durable publication commit and the final database path is
conservatively excluded from destructive recovery. Runtime readiness and AM's
Installing/Ready transitions occur later. Ordinary runtime startup does not use a
new-marker-absence assumption to classify old retained homes.

The integrated release qualification uses the explicit local command
`--qualify-first-publication-interruption`. It takes the real assignment receiver
path and stops after closing/syncing the staged DB, before creating the intent.
The process emits fixed boundary evidence and SIGSTOPs; the fixture must prove the
stopped lifetime, staged files, absence of intent/manifest/final DB and same-PVC
custody, then kill it and restart the ordinary `--wait-for-assignment` entry.
There is no environment, remote request or file-content trigger in ordinary
startup. This CLI is supplied by the integration component; the recovery component
contains only its post-sync hook. Native error-injection tests are separate from
actual release-container/PVC qualification and do not establish power-loss safety.

The V2 manifest/schema and wire errors do not change. Older readers still refuse
an absent manifest accompanied by its V2 artifacts. They do not understand the
intent and may open a committed manifest that retains one; if such a reader admits
effects, a later new reader refuses the nonempty proof rather than treating it as
fresh. Preserve the exact writer/reader identities during qualification and do not
claim rollback compatibility for this intermediate state without execution.

The branch adds tests for each durable publication boundary, repeated recovery
interruption, all effect/accounting tables, exact generation/schema/store checks,
malformed/foreign/unsafe intent, unknown/retirement/journal state, and actual
Supervisor installation only after intent consumption. Additional tests cover
no-intent empty creation/staged barrier prefixes, every cleanup boundary,
committed/final-state preservation, all-table residue, unknown/unsafe/retirement
state, path substitution and actual Supervisor installation after recovery.
Ambiguous final-DB and G+1 refusal assertions remain. Process-loss checkpoints and
manually truncated intent exercise retained states; they do not claim a real
filesystem power-loss or failed-fsync experiment. Compilation and execution need
the assigned native lane before any PASS claim.
