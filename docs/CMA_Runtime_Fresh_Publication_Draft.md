# Draft: complete an interrupted fresh runtime ledger publication

This branch is a conditional incident candidate. It is not a qualified repair or
permission to publish an image. Select it only after the real legacy-AM diagnostic
identifies interruption of fresh V2 publication in the covered window. A root,
device, ownership, mode or durable-I/O defect requires its own producer fix.

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

Existing 7a/10c5 partial publications lack this intent and remain held. An
interruption before the new staged database and intent are durable also remains
held. A malformed or partially written intent is not repair authority. This draft
therefore prevents recurrence in a bounded later publication window; it does not
automatically salvage the incident's existing retained PVCs.

The V2 manifest/schema and wire errors do not change. Older readers still refuse
an absent manifest accompanied by its V2 artifacts. They do not understand the
intent and may open a committed manifest that retains one; if such a reader admits
effects, a later new reader refuses the nonempty proof rather than treating it as
fresh. Preserve the exact writer/reader identities during qualification and do not
claim rollback compatibility for this intermediate state without execution.

The branch adds tests for each durable publication boundary, repeated recovery
interruption, all effect/accounting tables, exact generation/schema/store checks,
malformed/foreign/unsafe intent, unknown/retirement/journal state, and actual
Supervisor installation only after intent consumption. Existing no-intent partial
publication and G+1 refusal assertions remain. Process-loss checkpoints and
manually truncated intent exercise retained states; they do not claim a real
filesystem power-loss or failed-fsync experiment. Compilation and execution need
the assigned native lane before any PASS claim.
