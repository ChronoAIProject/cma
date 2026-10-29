# Event media continuations

Manager publishes an original event attachment into the same retained Agent environment and admits a separate supplementary response when the original event was already accepted. The original external-event command and its response remain immutable. Supplements use ordinary Append ordering, so a working original turn continues. This interface grants no general file upload, caller-selected Agent, filesystem path or arbitrary message capability to a binding key.

## Authority and retention

The caller presents the existing bound consumer Agent key. Manager resolves the original command through the permanent binding/conversation/event index, verifies current binding authority, and uses that command's Agent and nonzero authorization grant revision. The optional original Gateway binding generation is separate: a Manager generation of 3 with an absent Gateway generation is valid.

Every upload, receipt read, supplement admission and deferred input dispatch asks the configured Gateway origin to attest the retained event reference and exact durable artifact claim. The request includes the complete original Activity digest, binding/channel/conversation/event identity, original optional Gateway generation, exact original capability expiry and artifact. An event-only positive attestation cannot authorize bytes or text. Requests have a five-second bound, disable redirects and use no additional credential. Gateway refusals revoke the effect; an unavailable peer permits retry without a new identity.

The artifact pins attachment index, kind, original filename/MIME/size, original byte SHA-256, content SHA-256 and optional final command-text SHA-256. The descriptor always describes the original file. Derived content hashes exact UTF-8 transcript or excerpt bytes; the final text hash includes Manager's verified stored path. The services independently implement the same exact text wire contract without subsequent text normalization. Manager compatibility tests compare its DTO, complete Activity digest and text against the Gateway protocol through a development-only dependency; the deployed services remain separate.

Manager retains one immutable upload intent and successful receipt per original parent command and attachment index in `binding_event_media_uploads`. It stores no file bytes or credentials. A pending intent cannot authorize a path-bearing supplement. Runtime publication uses the deterministic attachment identity; a lost outcome retries that same identity. A completed receipt can be read without refetching source bytes. If publication is still pending and the source bytes are no longer retrievable, the receipt remains unresolved.

## Public routes

All paths share this base, whose conversation is the original transport conversation:

```text
/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}
```

`POST {base}/attachment` accepts binary bytes with required Content-Type and Content-Length, and query fields `name`, `sha256`, `event_expires_at`, and optional `gateway_binding_generation`. The name is the frozen sanitized ASCII filename. There are two concurrent body slots, a 20 MiB byte ceiling and a 45-second body deadline. Manager authorizes before buffering and checks exact length and SHA-256 before publication. A 201 receipt contains `id`, `agent_id`, `name`, `path`, `size_bytes`, `media_type` and `sha256`.

`GET {base}/attachment` needs no query or body and returns the exact durable completed receipt with 200. Missing or pending receipts return 404. Current binding and exact Gateway artifact attestation remain required. All receipts and refusals are private/no-store.

`POST {base}/supplement` accepts this strict shape:

```json
{
  "gateway_binding_generation": null,
  "event_expires_at": 1790000000,
  "artifact": {
    "attachment_index": 0,
    "kind": "transcript",
    "content_sha256": "<SHA-256 of exact derived text>",
    "source_sha256": "<SHA-256 of original file bytes>",
    "name": "voice.ogg",
    "media_type": "audio/ogg",
    "size_bytes": 123,
    "text_sha256": "<SHA-256 of complete canonical supplementary input>"
  },
  "derived_text": "Exact transcript text"
}
```

Kinds are `original`, `transcript` and `text_excerpt`. Original requires empty derived text and equal content/source hashes. Derived text is bounded at 64 KiB. Manager reads its own successful upload receipt, reconstructs the complete input, verifies both content and final text hashes, and admits Append. There is no caller-controlled command metadata or ExternalEvent in this new command.

A 202 response contains `command_id`, `response_id`, `response_url`, `status`, `queue_deadline` and `metadata`. The same metadata appears in normal Responses replay:

| Metadata key | Server source |
| --- | --- |
| `cma_parent_command_id` | Original permanent command receipt |
| `cma_parent_response_id` | Original permanent response receipt |
| `cma_parent_fingerprint` | Original command fingerprint |
| `cma_artifact_id` | Exact index/kind slot, such as `0:transcript` |
| `cma_command_fingerprint` | New canonical supplementary command fingerprint |

The slot is the original parent plus index/kind. Changing the digest does not create a new slot: exact retries return the same receipt and different content conflicts. Gateway retains a separate supplemental-response journal, validates this exact metadata and Agent identity during replay, and delivers that response separately from the original response.

Only supplements receive the longer queue lifetime: the lesser of attested original event expiry and 24 hours after admission. Ordinary commands retain their existing queue lifetime. Source authority is rechecked before admission, after durable reservation, during deferred authorization and at the ordered runtime writer boundary. Expiry or revocation cannot be bypassed by the longer deadline. The queued-input editor cannot rewrite a continuation.

## Durable retry before input publication

A transient source outage at the final ordered runtime writer may defer only the supplementary command. The runtime adapter returns a distinct input-authority-unavailable category before any write. In one Agent transaction, settlement records the exact operation as `PreWriteFailed`, appends the immediately adjacent `TurnFailed` proof with the same timestamp, and returns the Agent to Ready. The proof binds the command ID, canonical command fingerprint, continuation provenance hash and operation/turn sequences. A generic failure message or uncertain write is insufficient.

Reconciliation validates the full durable log and archives that exact failed dispatch before queuing the same command and response again. It retains the original permanent slot, upload receipt and source proof. It transactionally fences current Manager binding/conversation authority, the Agent revision and command/lane revisions, and refuses an expired command or stopped Agent. A concurrent cancellation remains terminal. Recording queued bookkeeping does not require Gateway availability and performs no input IO; every subsequent readiness, execution and final ordered write obtains fresh exact artifact attestation. A source revocation is terminal.

The first deferred retry waits two seconds. Subsequent delays double through 256 seconds, with at most 256 retained deferred attempts and the original admission deadline still enforced. The supplement keeps its original FIFO position during backoff; controls remain available. Missing or unknown no-write commits retain the current dispatch for reconciliation and never clear uncertain delivery. Unknown requeue commits recover from the same durable records without another command. Responses projects the exact paired internal no-write attempt as queued while requeue is pending, then uses the next attempt's actual turn; it does not emit a terminal failure for that eligible deferred attempt.

## Storage and rollout

The existing Manager schema initializer creates `binding_event_media_uploads`. Its unique `_id` index is sufficient; no TTL or additional index is needed. The Manager runtime role includes this collection using its existing collection-scoped actions. Existing shared-schema deployments use their existing permissions. Isolated deployments must apply the reviewed updated Manager role before enabling this interface; no additional credential, service or environment variable is required.

Ordinary retained command serialization is unchanged. Continuation commands contain a new optional provenance object and may retain deferred attempt evidence in their work state. Once such rows exist, rollback must retain a Manager reader and worker that understands them; old strict readers cannot safely interpret those rows. Preserve upload receipts and command rows rather than removing accepted work. Deployment and native provider acceptance remain separate operational actions.
