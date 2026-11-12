# Errors

<!-- nav: 10 Errors -->

All SDK failures derive from `CmaError`. `ApiError` represents a structured
non-success response, `ProtocolError` rejects an invalid response boundary, and
`TransportError` covers failures before an HTTP answer is available.

```ts
import { ApiError, CmaError, createClient } from '@chronoai/cma-sdk';

try {
  await createClient({ baseUrl: 'https://cma.example.com' }).readConformance();
} catch (error) {
  if (error instanceof ApiError) {
    console.error(error.status, error.code, error.requestId);
  } else if (error instanceof CmaError) {
    console.error(error.name, error.message);
  } else {
    throw error;
  }
}
```

`TimeoutError` and `RequestAborted` distinguish deadlines from deliberate
caller cancellation. Credential, request-id, and response-observer callbacks
also have dedicated errors.

`ApiError.extensions` retains structured RFC 9457 fields; `field`, `path` and
`reason` expose string extensions when supplied. Use `code` and extensions for
behavior, never prose matching. `isRetryable` identifies HTTP 429 and server
failures; it does not authorize creating a new command or changing its body.

Document writes report HTTP 429 with `document_limit_reached`,
`document_bytes_limit_reached` or `document_objects_limit_reached` when the
corresponding owner quota is exhausted. Their `ApiError.extensions` include the
authoritative `limit` and `live` values, with `live` reporting usage before the
attempted reservation. `document_capacity` remains the refusal
for other document capacity bounds; `document_write_busy` identifies temporary
write concurrency saturation and carries `Retry-After`. Read current owner
quotas with `client.usage.documents()` before presenting available capacity.

Profile, instance, Trigger-authoring, Launch and service-connection commands
require `idempotencyKey` containing 1 to 255 visible ASCII characters without
spaces. The SDK never generates keys or retries commands automatically. After a
timeout, lost response, protocol decoding failure or
`idempotency_in_flight` / `idempotency_recovery_required` response, keep the
original command body and key. A new key could duplicate admitted work. Follow
the service's documented recovery procedure; do not treat HTTP 409 alone as
proof that the operation was rejected before admission.

`repeatChoiceFrom(error)` extracts and validates the existing Agent context from
`trigger_repeat_choice_required`. Show that choice to the visitor; only their
explicit Resume or New selection creates the next command with a fresh key.
Other refusals must not become an implicit retry or default choice.

Trigger `save` and `publish` each have their own command key and exact
`draft_version`. A successful Save followed by an uncertain Publish means retry
the Publish, not the Save. Keep edited content during a version conflict, read
the current document, then let the owner choose how to proceed.

AG-UI `run` is an exception to the REST command-key shape: it submits one public
run request and never resends it. `runId` is correlation, not proof that repeating
the POST is safe. A run stream ending without `RUN_FINISHED` or `RUN_ERROR` has an
unknown outcome. Recover with read-only `agui.replay` and durable resource reads;
never manufacture a failed run because its transport was lost.

AG-UI replay reconnects only its GET, with bounded backoff, and advances its cursor
only after the event callback resolves. A rejected callback is not acknowledged.
Live `cma:live` content has no durable cursor and cannot be recovered by replay.
Launch `stream` similarly acknowledges after the callback, but performs only one
connection; the application owns bounded reconnect or polling fallback. Preserve
the last successfully observed record during an outage.

Operator settings use revision compare-and-swap rather than command keys, and
operator retirement methods have no SDK idempotency-key parameter. An ambiguous
operator write requires a fresh read before the operator chooses another action.
No method's `isRetryable` result authorizes changing this recovery contract.
