# Fresh-event binding preparation

This implementation is a disabled foundation for a separate replacement operation after a channel-owned Sandbox was explicitly retired and its home released. It does not enable automatic replacement in production. Ordinary binding PUT, conversation PUT and background recovery continue to preserve the original hierarchy. An entirely retired binding remains terminal.

## Retained authority and transaction

`BindingPreparationIntent` pins the durable event admission, target, request hash, owner, management principal, binding generation/revision and exact binding/Sandbox/Workspace digests. Its operation ID is a content hash of admission ID, target ID and binding ID. A new reservation requires a live admission whose timestamp is strictly later than the original Sandbox retirement. An event timestamp supplied by a producer is not an admission timestamp.

Only the dedicated `MongoBindingCredentials::prepare_retired` transaction can persist a generation change. It requires a Failed/Idle profile binding, no borrowed Agent or previous environment, a fully retired original Sandbox with released home, settled credentials and no active lifecycle, Workspace or child work. A retained Ready Workspace row does not establish that the released home still exists. The transaction validates indexed conversation records, requires all retained conversations to be terminal, refuses active child work and cleanup reservations, and writes exact parent fences.

The same transaction stores an immutable `provider_binding_preparations` receipt containing the complete original binding, a single reserved next generation and its new Sandbox/Workspace IDs. It admits the existing supplied key to encrypted custody for the new Sandbox and leaves durable `ProvisionSandbox` work. Original Sandbox, Workspace, conversation and history aggregates are retained; no original PV is changed or recovered by this operation. The receipt collection has no TTL and one unique `(binding_id, generation)` index, in addition to the operation ID. It belongs only to Agent Manager's existing exact-schema role and initialization mechanism.

Replays require exact intent equality and the same credential hash. Unknown commits may read back only the exact immutable operation receipt. They must never refresh expectations, generate a new operation key or infer a failed reservation from a transport error. A different event racing the same original generation cannot create a second hierarchy. Replaying a committed receipt after later lifecycle changes returns that receipt without allocating again.

## Generation readers

The optional `ProviderBinding.generation_guard` is omitted on existing records. Once present, a binding requires a matching positive `binding_generation` in Responses v2 submissions. Generation is checked before source-user routing, transport alias resolution and command admission. Conversation creation also has a generation-pinned core entry point; the original unpinned entry point refuses guarded bindings. Existing binding admission fences continue to protect the exact Agent and conversation at command admission.

The generic binding store still rejects every generation change, removal/change of the guard, and insertion of a fabricated guard. The dedicated atomic operation is the only writer. Old request bytes and unguarded bindings retain their current behavior.

Gateway readers understand an optional `binding_preparation_version` on ordinary admissions, an optional binding preparation checkpoint on the execution journal and `binding_generation` on frozen turns. All fields are omitted on existing admissions and requests. The checkpoint records either a pinned current generation, the exact pending replacement intent, or the same intent with its immutable provider receipt. Journal CAS/readback confirms both intent and receipt writes after an acknowledgement loss; a different intent cannot overwrite the first. A frozen request must match the acknowledged checkpoint generation. Borrowed ExistingAgent targets keep their existing behavior even in a mixed-target admission.

This reader release refuses a new Pending admission that requests preparation before invoking legacy conversation creation. It can retain and replay a previously prepared generation-aware request. No production admission writer sets the version and no dispatcher performs a replacement yet.

## Compatibility and remaining activation work

No HTTP management preparation route, Gateway preparation writer or production flag is enabled by this foundation. Activation requires the following remaining integration work:

1. Deploy the compatible strict readers for the Gateway admission version, persisted preparation marker/receipt and frozen-turn generation before writing any of those fields. Every rollback image must support the same retained shapes.
2. Add authenticated current-owner/current-manager inspection and preparation endpoints. Request owner/manager fields are comparisons, never credentials. Returning an old operation receipt still requires current management authority.
3. Mark only new verified ordinary user events as eligible. Interactions, callbacks, edits/deletes/reactions, generated messages, tests, legacy admissions and ExistingAgent targets cannot trigger replacement.
4. Under the existing Pending journal lease, validate the current enabled source/consumer/provider, persist the exact intent before network effects, and persist the exact receipt before conversation preparation. Unknown marker writes and provider responses require exact readback/replay. Prepared, Starting, Unknown and accepted requests never choose a replacement hierarchy.
5. Freeze and transmit the generation for every newly prepared turn. A migrated binding must reject legacy markerless work instead of rerouting it into its new hierarchy. Disabling further replacement must retain compatible readers and generation-aware traffic.

These changes require no NyxID code, agent-sandbox controller or infrastructure development.

## Verification

Focused domain and routing tests cover unchanged old serialization, generation-change refusal, exact generation submission/replay and rejection before conversation allocation. Gateway journal tests exercise lost intent and receipt acknowledgements, expired lease takeover, immutable expectations and receipt-before-freeze ordering. Disposable-replica-set tests cover identical and distinct concurrent requests, full rollback after archive/custody writes, lost application acknowledgement with store reopen, preserved original aggregates/history, corrupt or foreign child records, cleanup reservation refusal and retirement races. Both indexed and retained-record parent identity paths are indexed and checked, so a malformed lookup projection cannot hide a child. All Mongo fixtures use unique temporary databases and remove only those databases.
