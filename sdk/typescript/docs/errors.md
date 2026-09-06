# Errors

<!-- nav: 4 Errors -->

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

Every mutation requires `idempotencyKey` containing 1 to 255 visible ASCII
characters without spaces. The SDK never generates keys or retries commands
automatically. After a timeout, lost response, protocol decoding failure or
`idempotency_in_flight` / `idempotency_recovery_required` response, keep the
original command body and key. A new key could duplicate admitted work. Follow
the service's documented recovery procedure; do not treat HTTP 409 alone as
proof that the operation was rejected before admission.
