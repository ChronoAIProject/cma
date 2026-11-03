# Browser control contract and compatibility

Ticket 17 adds control of the already enabled Agent browser. It neither creates
another browser nor changes the Sandbox service scope, Agent permissions,
approval policy, conversation, task, filesystem authority or credential grants.
Only an owner with current ManageSharing authority may mutate browser control;
shared and read-only callers cannot acquire, renew or send input. ViewHistory
allows an authorized bounded audit/status read. Every request reauthorizes; a
lease is never a substitute for account or resource authorization.

## Runtime arbitration

The existing private browser mediator serializes control, native navigation and
native input on one queue. Native commands only reach the Agent socket; lease
commands arrive through the private runtime transport and inherited pipe. CDP
stays on inherited pipes. The public input vocabulary contains bounded browser
clicks and text, never arbitrary script, shell, CDP or a desktop/PTY channel.
This bounds the supported browser API. It does not add OS isolation between
arbitrary processes already sharing the Sandbox uid or claim to contain a
hostile unrestricted shell process inside that same uid.

An acquisition receipt binds runtime generation, boot, Agent, browser context,
authenticated actor/credential identity, private client key, client, opaque lease token and monotonic
revision. Competing clients and old tokens fail at execution time. An input's
expected revision also fences queued duplicates. Returning invalidates user
input without navigation, context replacement or conversation input. Native
commands can then continue in the original context.

Each client generates an independent random UUID key, sent only in the
`X-CMA-Browser-Client` header. Core hashes the key with the authorized account and
credential identity before forwarding it to the runtime. Public client ids and
operation ids in audit do not authorize receipt recovery. Mutations and selected
receipt reads require the original private key as well as current owner access.
Neither audit nor receipt URLs contain this key. SDK diagnostics redact it.

Leases last 15 seconds on a monotonic clock. Only an authorized exact holder
can renew. Input does not implicitly renew. An offline, logged-out or revoked
caller cannot renew through the public service; subsequent public writes fail
authorization immediately. The runtime releases the unattended lease within
15 seconds of its last accepted renewal, after any already serialized input
has settled. This is a bounded revocation window for Agent resumption, not a
claim that an out-of-band account change synchronously reaches the Sandbox.
Old user input never runs after expiry. Runtime/mediator replacement changes
the browser context or runtime incarnation and rejects the old identity.

## Receipts, uncertainty and audit

Clients retain the exact operation id and request while a reply is uncertain.
They query that operation or resend the identical body; they do not generate a
replacement acquisition/input/return. A receipt reports both the operation's
revision and current state. A lost acquisition receipt does not allow Agent
input after runtime acquisition. A lost return receipt does not keep an old
user token valid. The UI shows unknown/transitioning and sends no user input
until a correlated receipt or current-state query resolves uncertainty.

The Web keeps an exact pending request in memory. Session storage retains only
its scope, private client key and operation id, never entered text or the lease
token. After reload a new controlling client identity queries the old operation
without adopting that old lease; it waits for return or expiry. Closing an idle
holder attempts an explicit return. Hidden/offline views stop renewal and input.

The runtime retains 128 receipts and 128 metadata-only audit entries per
browser context. It never records input contents or lease tokens in audit.
Revision fencing prevents an evicted operation from being executed anew;
unknown/evicted receipt queries are explicit conflicts. Audit reports its
truncation boundary. This is publicly verifiable live-incarnation evidence,
not indefinite archival history or a replay of browser frames.

## Reader floor and rollback

No Mongo record or durable lifecycle schema is added by 17. Browser control,
receipts and audit are volatile within the existing mediator incarnation.
Restart is not same-context reconnect. After runtime replacement, clients must
read the new environment and explicitly enable/acquire a new browser.

The added private `browser_control` frame is capability-probed; an old runtime
that does not support it must not advertise available control. This ticket
does not add a field to RuntimeStatus. The ticket-15-aware reader floor remains
mandatory even with Browser off: the original strict baseline decoder rejects
15's `agent_browser` status field. Parent independently reproduces that decoder
incompatibility; this document does not claim old-binary restart evidence.
Ticket 16's creation journal floor and earlier admitted-operation floors also
remain required when those operations exist.

An API/Web rollback must retain the running runtime and a 15-aware reader.
A client without 17 control support cannot renew an outstanding lease; expiry
safely returns Agent control. Re-upgrade cannot revive the old token. Rolling
back or restarting the runtime while work is active is not authorized and
cannot preserve the browser context. Release 19 must verify its exact retained
runtime/API/Web combination and actual restart/recovery behavior.
