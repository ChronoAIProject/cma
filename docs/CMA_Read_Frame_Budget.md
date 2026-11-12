# Retained expensive-read frame admission

Part of #1472; follow-up to #1596/#1612. The independent strict boolean
`CHRONO_READ_FRAME_BUDGET_ENABLED` defaults to `false` in code and manifests.

## Ownership after body EOF

An HTTP body can yield its final `Bytes` frame and finish while the transport
still holds that frame. A permit held only in body state is released at EOF or
body drop, allowing another expensive read while the previous payload remains
retained. Zero-copy frame clones or slices can extend that lifetime further.

Enabled mode moves the existing admission permit into the backing owner of the
returned `Bytes`, using the existing `bytes::Bytes::from_owner` API. No payload
copy is added. The permit survives body EOF/drop and all zero-copy clones/slices;
the last owner drop releases both storage and admission. Holding even a small
slice keeps the original full response reservation. Empty responses release
immediately because no response payload remains to account for. Cancellation
before publication and size/body/timeout failure also release their slot.

The existing two-slot ceiling, 64 MiB response payload limit, 256 KiB artifact
metadata limit, deadline, route inventory, pre-handler typed 429 and protected
control paths remain in place. A third read while both buffers are retained is
refused without invoking its handler. Stop/cancel/revocation retain their
existing authorization and admission rules. OFF preserves body-owned permits.

This is a lifetime correction for admitted public response payloads. It does
not measure allocator capacity, decoder/store working memory, independent deep
copies, TLS/socket buffers or total process RSS. Shared ingress, replay working
memory, fairness and fleet mapping remain separate #1472 qualification. A new
owner does not make unrelated copies share this reservation.

## Rollout and evidence

There is no persistent format, migration, new writer, index or wire change.
All source gates run on local socket-free HTTP fixtures; no Sandbox, model turn
or production request is required to prove this ownership contract. Tests hold
returned frames across body EOF/drop, retain clones/slices, check refusal before
handler effects and protected controls, then drop final storage and prove reuse.
They also cover collection, unread bodies, empty/denied responses, malformed
stream/oversize, timeout/cancellation, strict configuration and OFF behavior.

Release owners enable only a qualified candidate/configuration through a staged
restart. The stronger lifetime may expose more truthful capacity refusals when
transport writes are slow; do not increase limits to hide them. Rollback to OFF
restores the previous lifetime policy and needs the usual release authority;
it does not delete data or acknowledge pending work. Existing readers/handlers
remain compatible, but this PR does not certify a deployed image or RSS budget.

A later coordinated security-harness control may read existing authorized small
resources: at most two observers, ten requests and two minutes, no new resource
or model activity. It can verify ordinary success/recovery and aggregate
refusals, but cannot prove a hidden transport-buffer lifetime by itself. Exact
candidate, effective flag and request outcomes belong in the release receipt.
Abort on an owner-control failure, unexpected 429/503, candidate drift, private
data or a health regression. Close bodies and tracked sessions, reconcile the
fixture ledger, and retain the first failure. Do not send shared-service load
or change configuration without the separately relayed campaign authority.
