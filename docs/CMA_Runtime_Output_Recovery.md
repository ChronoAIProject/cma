# Runtime output acknowledgement

The hosted conversation transport can retain native output until the API has committed its durable effect. This protects a live native conversation across WebSocket disconnection and API replacement, including a terminal result whose database commit succeeded but whose acknowledgement was lost.

## Negotiation and compatibility

The API requests `?outputAck=1` on the existing runtime conversation WebSocket endpoint. Wire version remains 1. A supporting runtime advertises:

```json
{"outputAcknowledgement":{"version":1,"streamId":"d99003d1-0c3c-46cd-aa47-9bbdbed50b12"}}
```

The first attachment pins the Bridge's mode. A Bridge first used by a legacy API remains on the legacy conversation lane even when a later API requests acknowledgement. Its previously accepted output has no durable cursor and cannot safely be replayed into a later turn. A Bridge first used in acknowledged mode rejects a legacy downgrade before WebSocket upgrade with `409 output_acknowledgement_required`.

Older runtimes ignore the query and omit the capability. Existing runtime attachments continue using their established transport behavior. They do not gain retained output or recover historical missing results through an API update.

## Wire positions and durable acceptance

| Lane | Direction | Payload |
| --- | --- | --- |
| `0x02` | API to native; legacy native to API | Opaque ACP frame |
| `0x03` | Runtime to API | Existing explicit conversation-gap declaration |
| `0x04` | Runtime to API | Stream UUID as 16 bytes, cursor as unsigned 8-byte big-endian integer, then opaque ACP frame |
| `0x05` | API to runtime | Exact stream UUID and cursor header, without ACP payload |

The UUID is stable for the held Bridge. Cursors start at 1 and advance for every native stdout frame. Retention happens before delivery to any consumer. Reconnection snapshots the retained journal and continues from its notifications; the legacy broadcast queue's 256-frame limit does not cause gaps in this path.

The API resolves bootstrap RPC replies immediately while holding domain output until the handshake verifies the native conversation. It then emits receipt-bearing updates in cursor order. A protocol-only frame receives an ordered no-op receipt. Pre-session owner sign-in notices retain the existing startup publication behavior so authentication can complete; their raw frames are acknowledged only after session activation, and startup notices do not acquire new transactional deduplication semantics.

The private Agent message checkpoint stores the last accepted source position and payload hash, scoped to the native conversation and runtime generation. Its transaction includes any assembled assistant text, Agent changes, and log entries for that frame. The position survives turn changes and unrelated semantic writes. Replay at or below the accepted position skips folding; this also covers an old terminal that the new adapter parses as a no-op because the prompt already completed. A skipped cursor, changed current payload, foreign native identity, or unexpected stream is refused.

The CMA update consumer sends the cumulative acknowledgement only after durable recording succeeds. Queuing, parsing, live publication, and a WebSocket write are not acknowledgement points. A lost acknowledgement causes replay and durable deduplication.

## Bounds and operational limits

Each Bridge retains at most 8 MiB including per-frame metadata, and at most 16,384 frames. Exceeding either limit permanently invalidates reliable delivery for that stream. The runtime reports an explicit gap and closes the reliable link; it does not silently evict old output, report a successful terminal, or terminate the native process. Valid retained prefixes can still be acknowledged, but the gap cannot be repaired by reconnecting.

Retention is in the runtime process's memory. It is not a journal that survives runtime Pod/process replacement. The original Bridge remains directly reachable after native stdout exits, but the adapter still requires a successful ACP handshake for domain activation; transport replay alone does not establish end-to-end recovery from a dead native process.

Deploy compatible API readers everywhere before enabling this runtime image for future runtimes. The private checkpoint field is optional for existing documents, but older API binaries reject documents containing the new field because their private decoder denies unknown fields. After acknowledged output is in use, rollback must retain readers that understand that private field. Retained user runtimes do not need rotation to deploy the additive API support.

## Focused verification

The runtime tests cover disconnect replay, cumulative ACK validation, 1,024-frame backlog delivery, byte/frame overflow, terminal transport replay, and both legacy mode transitions. Adapter tests cover a 302-frame backlog whose terminal precedes handshake completion. Core tests cover lost commit replies, lost acknowledgements, no-op cursor persistence, foreign and skipped positions, and old-terminal replay after a later turn. CMA tests enforce no acknowledgement before durable success. Mongo checkpoint tests exercise cursor persistence and exact replay after reopening the store.
