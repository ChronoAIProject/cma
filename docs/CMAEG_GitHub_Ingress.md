# Native GitHub receipts and authority

This document describes the installation-authority release. It supersedes the
former file-backed proof, static route, execution-group, batching and Agent-result
parsing instructions. [The protocol](CMAEG_Protocol.md) is normative; see
[the design](CMAEG_GitHub_Integration_Design.md) for the staged ordinary-dispatch
work and [the online runbook](CMAEG_GitHub_Online_Verification.md) for acceptance.

## Receive contract

`POST /v1/github/apps/{app_ref}/webhooks` selects the persisted integration and
its sealed current/overlap webhook credentials. It verifies HMAC-SHA256 over the
exact received bytes before parsing or duplicate lookup. Single-valued delivery,
event and signature headers are required. Media must be uncompressed UTF-8 JSON,
at most 256 KiB. The handler has a five-second budget and never awaits an Agent.

The identity is `(github.com, app_ref, X-GitHub-Delivery)`. An exact duplicate
returns the retained receipt; changed bytes or event header return 409. The HMAC
authenticates the body, not those headers, so supported payload shapes must agree
with the claimed event. Fork PRs use the top-level installed repository.

| Condition | HTTP and durable outcome |
| --- | --- |
| Retired integration | 410 `github_integration_retired`; no new receipt, including ping |
| Valid ping | 200, ignored receipt, no tasks |
| Unbound installation | 200, compact terminal `installation_unbound` receipt |
| Pending/suspended/uninstalled binding | 200, terminal ignored business receipt with reason |
| Removed or unapproved repository | 200, ignored receipt; no execution authority |
| Repository owner mismatch | Persist exclusion and authority revision with ignored receipt |
| Lifecycle delivery | 200 after authority/receipt transaction; no business task |
| Unsupported business event/action | 200, ignored receipt |
| Approved eligible business delivery | 202 with a committed pending receipt; bounded dispatch reports per-channel progress |
| Invalid signature/body/media | 401/400/415; no trusted receipt |
| Oversized body | 413; no truncation into success |
| Unavailable or uncertain storage | 503; recover using the original identity |

Signed lifecycle events remain receivable while a binding is inactive. Positive
notifications do not activate it or re-include repositories. Explicit admin
re-approval is required. Uninstall is terminal. A duplicate lifecycle delivery
cannot increment authority twice.

## Persistence and query

Ignored business/ping bodies are encrypted interaction chunks retained at most
24 hours; lifecycle bodies are retained at most seven days. Ignored receipt roots
retain seven days. Business receipts with pending dispatch retain fourteen days so
an admission committed during recovery has its full seven-day execution horizon.
Receipt roots use the existing `expires_at`/TTL pattern.
Chunks precede the receipt root. A failed commit may leave expiring orphan chunks,
never a successful partial receipt. Compact ignored receipts may retain no body.
The public response never contains raw chunks or credentials.

`GET /v1/github/deliveries/{receipt_id}` and
`GET /v1/github/apps/{app_ref}/deliveries/{delivery_id}` enforce the retained owner,
including after retirement. Retired bindings preserve their recorded history but
authorize no work. A newly verified integration can request and independently
approve the same installation with its own binding ID; it inherits neither the old
approval nor its receipts. Other owners receive 404. Delivery ID conflicts
never overwrite the original receipt. A 404 during an uncertain request is not
proof that an in-flight transaction cannot later commit.

Binding changes and lifecycle receipts commit together. Authority mutations keep
integration-version-conditioned write guards, fencing retirement and credential
rotation; inert ignored/ping receipts take no such guards. Conflicts use bounded
jittered backoff within 3.5 seconds. Approval commits audit/idempotency atomically.
Management CAS version and effective eligibility revision are separate: unrelated
repository events and already-applied exclusions do not advance authority revision.

Version 5 storage is prepared through the existing flag-gated startup migration.
Unknown versions and unreadable legacy records fail closed; no static config
loader or legacy execution worker remains. Receipt reads/replay return 410 after the recorded deadline while the row remains,
then 404 after TTL removal. Ignored duplicate recovery is bounded to seven days;
pending business recovery is bounded to fourteen days and never
renews the deadline. Legacy receipts receive one fixed seven-day migration stamp. The current release does not reconcile missed lifecycle
events through the GitHub API.

## Execution boundary

`launch_github` uses ordinary durable routed admission, one Event per delivery and
matching channel. Receipt progress, admission identity and execution outcome are
separate facts. `Task.result` remains a v1 compatibility field and is never filled
from Agent replies. Gateway task completion is not proof of a PR merge or Pages
deployment. No execution groups, batching window or special board command exists. Recovery is
owner-scoped and bounded to 64 records per owner page, 16 dispatch attempts per
five-second tick, exponential backoff capped at 1,280 seconds, and a 30-second
Mongo lease fence. The existing `(owner, kind, _id)` index supplies the recovery
page; no other owner's delivery rows are scanned for an owner.

Run receiver regressions with `cargo test --locked -p cmaeg --test github_source
-- --ignored --test-threads=1` and process tests with `cargo test --locked -p cmaeg
--test management github_integrations:: -- --ignored --test-threads=1`, using the
existing authenticated disposable Mongo contract fixture. Its test-only Mongo
failpoint support permits scoped write-delay races; runtime credentials do not
gain administration permissions.
