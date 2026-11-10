# Agent session link rollout

Deploy Agent Manager before enabling the Web share-link controls. Agent links are
additive records in the existing `resource_shares` collection, with an optional
`record.link` document. Legacy organization grants omit this field and retain
their current behavior; no aggregate, Sandbox, PVC, conversation or runtime
identity is rewritten. The existing resource-policy transaction atomically
updates the link, policy fence and immutable command receipt.

Agent Manager creates the additive `resource_share_link_recipient` index over
`accepted_subjects`, `revoked`, `selector_kind` and `selector_id` on startup. The
existing runtime database role already owns the collection/index lifecycle; no
infra service, database or agent-sandbox change is required. Wait for all old
Agent Manager replicas to terminate before enabling link creation. An old image
cannot interpret new typed link records, so a rollback requires a compatible
Agent Manager build, not deletion of share data.

The browser generates a 32-byte cryptographic random token and keeps it in the
share URL fragment. REST requests send it only in the create/redeem JSON body.
Agent Manager persists its SHA-256 digest only; management/list responses never
return raw secrets or digests. A public link still requires CMA sign-in and
possession at redemption. Organization/user audiences additionally validate
current membership or canonical directory identity. Accepted users are recorded
per link. Every later API request and stream policy fence uses the current share
record. Revoking one link does not revoke another independent grant.

Agent view/write links allow interaction and selected Agent configuration/file
operations. They never grant Sandbox lifecycle management, sharing management,
or owner credential custody. Runtime-managed credential/control files must be
excluded from filesystem access for shared recipients. Existing owner and legacy
organization sharing behavior remains unchanged.

Raw native history downloads (`GET /api/v1/agents/{id}/history`) are restricted
to the personal owner because the native format can contain private metadata.
Shared readers continue to use the projected transcript and event endpoints.
Downloads retain the initial authorization across provider I/O and recheck the
policy, authority lifetime, ancestry, native conversation and runtime binding
before releasing buffered bytes. Changed or unavailable authority fails closed.

Production acceptance checks: owner creates independent view and view/write
links; wrong token and wrong recipient fail; view permits history/files but no
interaction or mutation; write permits interaction and supported configuration/
file operations; organization membership loss fails closed; revocation blocks
new requests and the next stream fence; another active link remains usable;
shared Agents appear only in Agent inventory, never shared Sandbox inventory.
