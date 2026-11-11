# Same-image retained Sandbox recovery

Part of #1575 and related to #1448. The owner can explicitly request reopening
of original conversations on an already restored, installed and credential-ready
runtime. The route preserves the recorded immutable image, retained home, files,
Sandbox, Workspace, Worktree, Agent and conversation identities. Managed image
update selection is separate and continues to exclude custom images.

## Activation and owner action

`CHRONO_RETAINED_REOPEN_ENABLED` is a strict boolean: absent or `false` disables
new admissions; `true` enables the explicit owner POST. Any other value fails
startup. The manifest defaults to `false`. The flag does not schedule recovery.
Only the release owner changes production configuration after review and artifact
qualification. Turning it off blocks new requests but retains accepted operations
and permits exact-key receipt retries.

After deployment and activation, open the existing Agent's **Sandbox details**,
click **Recover on the same image**, then **Recover my Sandbox**. Review the
current Sandbox before confirming. A read-only assessment precedes the dialog;
POST repeats stronger proof and can still refuse. An accepted request appears in
the existing Sandbox progress. Return to the same Agent once it is Ready.

The browser stores the owner/Sandbox-scoped revision and request key in session
storage before sending. A lost response offers **Retry recovery request** with
the original revision and key, including after page reload or lifecycle progress.
It never turns a timeout into a new recovery attempt. Missing browser storage
blocks submission. SDK callers must retain their own original request key.

## Supported requests and consent

An authenticated owner needs `UseOwnSandboxes`. `ControlAnySandbox` alone does not
permit this route: the owner consents for their own Sandbox. Foreign resources
return 404 and missing capability returns 403. GET and navigation never admit work.

```http
GET /api/v1/sandboxes/{id}/runtime-recovery

POST /api/v1/sandboxes/{id}/runtime-recovery
Idempotency-Key: <original-request-key>
Content-Type: application/json

{"expected_revision": 7, "confirm_same_image_recovery": true}
```

POST accepts only these two body fields. It derives image, native identity,
boot and proof from retained records and fresh observations. Success returns 202
with `operation_id`, `revision` and `accepted_at`. Repeating the same accepted key
and expected revision returns that immutable response after progress or gate
closure. A different key/revision conflicts. There is one accepted receipt per
retained Sandbox; it is never overwritten to rearm a failed recovery.

Missing/malformed keys return 400, invalid consent/body returns 422, changed or
unproved authority returns 409, and disabled/busy/unavailable evidence returns
503. No raw native error, history content or home identity appears in responses.

GET returns a fixed `disposition` and the deployment's `admission_enabled` value.
This flag states availability of the explicit operation, not individual eligibility.

| Disposition | Meaning |
|---|---|
| `active_lifecycle` | The durable lifecycle is already Active; no native health claim. |
| `recorded_boot_present` | Exact current native/home authority and histories observed. |
| `exact_boot_replacement` | A different boot under exact retained image/home/generation authority. No permission to interrupt it. |
| `exact_suspended_runtime` | An empty child set and exact native graph proved suspended; outside this reopen slice. |
| `evidence_unavailable` | A dependency or bounded inspection did not provide complete evidence. |
| `authority_changed` | Parent, children, native graph, generation or home no longer matches. |
| `terminal_failure` | Parent or child is outside the supported recovery state. |
| `child_activity` | Work, dispatch, cleanup, stop intent or live native process remains. |
| `unsupported_image_authority` | Exact immutable image, contract or native home is unproved. |
| `provider_evidence_refused` | Native inspection refused; no narrower cause is invented. |
| `incomplete_history` | Original conversation histories cannot be proved. |

## Preconditions and boundaries

The parent must be Failed with `restore_failed` or `runtime_lost`, without work,
lease, operation, provision reservation, pending credentials, suspended binding,
incomplete managed update or a consumed automatic-recovery attempt. This failure
class is only a candidate: the fresh complete proof is mandatory.

Every retained Agent must be Paused and idle, with its original conversation
published on a strictly older generation. No queued or unfinished first input, uncertain
operation delivery, stop, cleanup debt or pending skill installation may remain.
Every Workspace and Worktree must be Ready and idle. The complete command
inventory must be terminal and command lanes unleased. Child counts are capped
at 16, retained commands at 4,096; larger sets refuse.

Each complete Agent log is structurally and semantically audited. The current
parent generation must have no publication or retirement in any child log.
The ordinary log capacity must have headroom for 64 entries and 512 KiB before
native reopening; cleanup reserves are not borrowed. Corrupt retained semantics
revoke a previously trusted checkpoint through the existing quarantine mechanism.

Native inspection proves the original Claim/Sandbox/PVC graph, immutable image,
home, current high-water generation and runtime contract. Status must show
installation and credentials ready, no restore pending and no Agent process.
Bounded provider inspection requires complete settled original histories and
zero Agent/unowned processes before and after history reads. Boot/home/native
changes refuse. Old unfinished turns refuse even if no process exists: they are
not rewritten, purged or replayed. The current script's pinned desktop services
remain permitted under their existing exact-source checks.

The initial native ledger gate supports the retained V1 format only. It requires
an unused current-generation process journal and authority scope: even a completed
old open cannot be replayed when its bridge is absent. Persistent journals must all
be completed with matching authority, with no pending/failed/indeterminate receipt,
cleanup record, temporary write or format barrier. At most 128 journal plus authority
rows and 16 MiB qualify, with at least 64 MiB of filesystem space available,
leaving conservative room below the V1 ledger limits for
all 16 children and their cleanup. A second inventory digest must match. Indexed
or partial ledger formats refuse until separately qualified; this route never
migrates a ledger or modifies a terminal receipt. Runtime-owned validation still
runs before each effect, and filesystem write failures remain possible after proof.

This slice does not suspend, replace, allocate or update an image. It cannot
recover an unavailable/uninstalled runtime, live children, terminal capacity
failure or uncertain native/history state. In those cases the saved environment
stays retained. The broader suspension/replacement and live-child settlement
scope of #1575 remains open; retries cannot substitute for missing proof.

## Atomic admission and continuation

Read-only proof has a 25-second bound and one concurrent request per API process,
without a waiter queue. At admission a majority transaction re-reads the complete
parent, child/worktree/command/lane inventory, event stream and audited history.
Both command and complete history digests are compared with the pre-inspection
snapshot. Writes fence every child, worktree and existing command lane; the
parent CAS serializes ordinary create/message/lifecycle admission. The transaction
stores the transition, usage boundary, status event and immutable receipt together.
An abort leaves no partial fences or receipt. An unknown commit is reconciled at
its exact snapshot time against the complete receipt, resulting parent digest and
event before returning acceptance.

The durable operation uses the existing `Revive / ConversationReopen` suffix on
the current restored generation. It retains the exact home-recovery identity and
expected/observed boot in the receipt. Existing workers adopt the bound runtime,
reconnect original conversations, publish under generation/lease guards, settle
queues and publish Active. Admission begins with no pending input to deliver.
A newly replaced boot must fail the bound-runtime fence before publication.

## Rollout, rollback and verification

The sole new persisted field, `retained_reopen_receipt_v1`, is top-level metadata
outside the serialized Sandbox `record`. It holds the source failure, owner,
revision/key fingerprint, complete snapshot/result digests, native proof and
accepted response. Existing aggregate readers decode `record`; existing `$set`
saves preserve the receipt. No lifecycle enum, event kind or serialized aggregate
field is added. Generic boot-successor checks remain unchanged; only the typed
proof-bound admission may record the freshly verified boot.

Before enabling, qualify the exact candidate and rollback artifacts against the
admitted existing lifecycle states and receipt preservation. Do not infer worker
compatibility from an opaque provider plan. If that qualification fails, keep
admission off and establish a compatible reader/worker floor first. Configuration
rollback never clears a receipt or an automatic one-shot flag.

Release verification records aggregate admitted/refused/completed counts, the
fixed `retained conversation reopening` outcome/duration fields, existing recovery
stage/reason telemetry and original-conversation publication. Public evidence
contains no identities. Protected checks establish unchanged image, home, files,
resource/conversation identities, no old-input replay and successful new owner
input after Ready. Stop activation for changed authority, uncertain history,
replay, an unexpected resource effect or a failed preservation check. Disable
new admissions and retain accepted evidence for supported reconciliation.

Production execution belongs to the release owner. Deployment, owner consent and
verification are separate from merging this default-off implementation. Do not
close #1575 without relayed production evidence and explicit disposition of its
remaining recovery shapes.
