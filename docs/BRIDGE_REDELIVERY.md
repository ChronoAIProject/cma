# Signed platform redelivery receipts

This follow-up is separate from the connector capability catalog and Telegram-new adapter. It is not live verification or mixed-writer rollout evidence.

The [disabled text-control foundation](CHANNEL_CONTROLS.md) adds a shared
Message/Control disposition collection for new callbacks. Both choices conflict
on the same verified callback/provider identity inside the admission transaction.
Existing Message receipts and provider aliases are still recovered first with
their original validators, preserving any accepted legacy reserved-prefix text.
The strict Message schemas described here remain unchanged, and no historical
provider identity or disposition is inferred or backfilled.

NyxID may assign a new callback message UUID when a platform delivers the same original webhook again. Ordinary callback replay remains exact: the original NyxID message ID and signed body must match the permanent receipt. A changed body under an already accepted callback ID remains a conflict.

For verified original human text, CMA also derives a provider identity from the owner, Bridge, configured issuer, selected bot, exact platform, actual chat, and provider message ID. It compares sender, conversation kind, verified thread evidence, raw-corroborated parent, content type, and exact text. Generic callback fields, display names, reply tokens, callback UUIDs, correlation IDs, and transport timestamps do not establish provider equivalence. Edits, attachments, actions, bot/loop events, and uncorroborated parents cannot produce the descriptor.

The first accepted message inserts its provider identity in the same majority-durable Mongo transaction as the normal inbox, rate, permanent receipt, and encrypted reply custody. Later matching callbacks persist an exact alias to that first acceptance. An alias creates no inbox message, allocation, command, capacity/rate charge, reply custody, or expiry extension. The response retains the first receipt and original command/reply destination. Changed semantic content under the same provider identity is a replay conflict.

After an uncertain commit, read-only reconciliation returns the original receipt only when the exact later alias is already durable. A matching first provider identity alone does not prove the alias committed. Missing or corrupt primary/receipt evidence remains unavailable. Current Bridge binding and custody remain required for both first acceptance and alias replay.

Provider identities and aliases are permanent records in `bridge_callback_platform_messages`, using separate `message:` and `alias:` unique `_id` namespaces. They retain hashes and immutable acceptance/source identity, not text, attachments, provider tokens, or interaction secrets. They remain useful after inbox/reply payload expiry and must not be deleted by callback payload TTL cleanup.

No historical provider identity is inferred from legacy receipts. An old writer does not populate or consult this collection; an old accepted callback replay does not backfill it. Therefore a later provider redelivery can still create a new accepted message across the old/new writer boundary. A coordinated writer handoff and issue 850 rollback/schema evidence are required before claiming deployment-wide deduplication. The first version performs no speculative history backfill.
