# Channel text controls: admission and execution foundation

Production Channel control ingress remains disabled for #854, with mixed-version
acceptance still tracked by #850. Production always constructs the dedicated
execution adapter so retained accepted controls can drain or reconcile while
fresh ingress is disabled. The admission adapter remains unconfigured and the
context tool advertises no control operations. Authenticated CMA dashboard/API
controls remain the available control path; approval capability and reply-tool
eligibility are unchanged.

After signature verification, trusted raw human/message/text classification and
the configured source-admission checks, CMA reserves the case-sensitive namespace
`CMA-CONTROL` at the beginning of ordinary text. Leading whitespace also reserves
the namespace but makes the request invalid. Embedded examples and arbitrary
chat text, including `yes` or `approve`, have no control interpretation. Native
Discord interactions, Telegram callbacks and Lark card actions keep their
existing unsupported classification; their normalized text cannot use this path.
The persistence boundary also refuses fresh transport-only writes that have not
completed trusted source classification. Such writes can still recover exact
existing receipts; signature verification alone cannot bypass the text guard.

The version-1 grammar is:

```text
CMA-CONTROL/1 cancel ccr_<reference>
```

```text
CMA-CONTROL/1 steer ccr_<reference>
Additional instructions for the original turn.
```

The header uses single ASCII spaces. A reference has a nonempty suffix of at
most 128 ASCII letters, digits, underscores or hyphens. Cancel has no payload or
trailing newline; steer has one nonblank payload after a newline, validated with
the ordinary Agent text-input rules. The entire reserved text is capped at
65,536 UTF-8 bytes. The production ingress returns the durable refusal
`channel_controls_disabled` for every otherwise valid cancel/steer request.

Malformed reserved text returns `invalid_channel_control_request`; a numeric
unsupported version returns `channel_control_version_unsupported`; other
operations return `channel_control_operation_unsupported`. Version-1 `approve`,
`approval`, `allow`, `deny` and `abort` are always `approval_dashboard_only`.
These refusals cannot reach ordinary Message admission. The response is:

```json
{
  "status": "control_refused",
  "receipt": {
    "version": 1,
    "callback_id": "<64 lowercase hexadecimal characters>",
    "bridge_id": "brg_example",
    "message_id": "nyx-message-id",
    "persisted_at": 1790000000,
    "code": "channel_controls_disabled"
  }
}
```

This receipt proves a persisted transport refusal. It is not an Agent command,
cancellation result, approval, Agent event cursor or delivery acknowledgement.
The transaction creates no ordinary inbox, reply-custody, outbox, command or
command-audit row for the Control. Fresh refusals consume ingress rate capacity
but never Agent queue capacity. Authentication, bounds, custody, persistence and
unknown-commit failures keep the established callback failure envelope.

New Message and Control classifications write the same version-1
`bridge_callback_dispositions` callback/provider identity in their existing
majority transaction. It retains only exact source identities, body/semantic
digests and safe receipts, without raw text, control references or reply secrets.
Exact retries and verified provider aliases return the original outcome without
extending retention, rate usage, reply custody or Agent order. Alias reconciliation
requires a durably recorded exact alias; provider equivalence alone cannot prove
an uncertain commit succeeded.

Existing permanent Message receipts, retained inbox rows and provider aliases
are recovered before considering a new reserved-prefix classification. Thus an
already accepted legacy Message containing this prefix remains that original
Message, including after its payload expires when permanent evidence exists.
No text-based reclassification or disposition backfill is performed. Existing
strict Message receipt and provider-alias schemas remain unchanged. An old
ingress writer without this guard can still admit newly received reserved text;
mixed-version and rollback acceptance remains an explicit #850 gate. This
foundation does not enable source control or claim mixed-writer safety.

## Accepted-intent receipt reader

Readers also recognize a separate version-2 retained accepted-control receipt:

```json
{
  "status": "control_accepted",
  "receipt": {
    "version": 2,
    "callback_id": "<64 lowercase hexadecimal characters>",
    "bridge_id": "brg_example",
    "message_id": "nyx-message-id",
    "intent_id": "cci_<64 lowercase hexadecimal characters>",
    "persisted_at": 1790000000
  }
}
```

This acknowledges durable intent acceptance only. It carries no fabricated
command/response identity and makes no claim that a runtime operation ran or
completed. HTTP recovery uses `202` with `Cache-Control: no-store`; the body
does not send a platform message.

Only `record.version: 2` in the shared disposition can contain
`control_accepted`; version 1 remains exclusively Message or ControlRefused.
The common outer `document_schema_version` stays 1. Readers validate the
receipt's version, original callback/Bridge/message identity, intent identifier,
provider identity and record digest. Exact and verified provider-alias recovery
return the original receipt without selecting Message work. Cross-version or
malformed retained records fail closed instead of admitting new work.

The accepted-control admission adapter commits the immutable intent, held common
command, original callback/provider acceptance records, and exact aliases in one
majority transaction. It shares the existing callback custody, replay, reply and
rate checks. A permanent version-1 refusal can never become an accepted intent
on retry or re-enablement. A retained intent without its original canonical
acceptance cannot be repaired by silently manufacturing new acceptance records.
The original authorization deadline is checked before each successful commit.

Execution requires fresh authority for the original human, source command,
consent, allocation, runtime and operation. Both canonical acceptance records
must agree with the complete immutable intent before releasing the hold, in the
conversation transaction, and immediately before the ordered runtime write.
That transaction also conditionally fences both canonical records against
concurrent mutation. An accepted control stays bound to its original turn and
does not start another response. An uncertain runtime write retains its recovery
state without automatic replay. Admission and execution create no outgoing chat
message.

Held controls also have a refusal-only transaction. A positively proved local
grant revocation or later complete consent generation (including revoke then
re-enable) ends the original held command with `authority_revoked`. A terminal
original source or changed turn/runtime on the same Agent incarnation ends with
`target_turn_changed`. The transaction rereads and fences the canonical callback
and provider acceptance, immutable intent/reference, source, consent history,
grant and Agent before settling the original receipt. Missing/corrupt evidence,
an uncertain remote authority check or storage failure preserves the hold.
Refusal uses the original bounded release attempt, never a renewed deadline;
unknown commit recovery rereads the original common command. Genuine queue
expiry remains separately typed as `queue_timeout`. None of these paths emits an
outgoing message or creates runtime input.

The existing `CHRONO_BRIDGE_EXECUTION_ENABLED` flag and the control's exact
platform flag gate only the transition from a held accepted control into the
shared command queue. Telegram and TelegramNew are separate platforms. Disabling
these flags preserves unexpired held controls and still permits genuine queue
expiration. Already released or claimed controls continue to require fresh
source authority; dispatch-bound commands can reconcile uncertain outcomes
without replay. No additional control-release flag is required.

Enablement requires a selected rollback baseline that understands version-2
receipts, counts held controls in queue capacity, and can execute or reconcile
already accepted controls with fresh admission disabled. The published reader
baseline alone lacks the held-control capacity accounting and dedicated executor.
Refusing an unknown receipt or merely preserving a held command does not establish
uninterrupted rollback. Cross-process fixtures exercise the earlier reader and
the combined compatible executor against the same actual persisted records;
production enablement remains gated on their completed validation and deployment
compatibility evidence. Constructor and database fixtures do not establish live
worker handoff: the production runtime registry is process-local, so retained
Agent, runtime and native conversation continuity still needs deployment evidence.

NyxID ignores callback response bodies, so this response does not send a chat
message. External messages require an actual `reply_to_channel` tool invocation;
channel-created Agents can invoke that tool during an active turn. This slice
adds no automatic final, waiting, status or refusal sends. Approval stays in CMA
under the actual authenticated operator's current authority, with no change
from Guarded to Autonomous.

The current reply tool requires an InProgress source turn. An approval-blocked
runtime may not be able to invoke another tool, and pending approval projects
WaitingAction. In that situation CMA's durable approval/status display is the
guaranteed fallback. An actual permitted explicit tool call may say, "This work
needs approval in CMA. An authorized operator must review it. Chat messages cannot
approve it." Include no private administrator link or hidden tool detail. Do not
manufacture a tool invocation from a control-plane approval observer. See
[Channel reply tools](CHANNEL_REPLY_TOOLS.md) for the complete authority boundary.
