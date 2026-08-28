# TypeScript Errors

<!-- nav: 70 Errors -->

Every SDK-owned failure extends `CmaError`. Service refusals preserve HTTP status,
stable problem code, and human detail; typed refusals add fields callers can act
on. The SDK never infers a type from provider prose.

```ts
import {
  ApiError,
  CapabilityRefused,
  DeploymentAtCapacity,
  RequestAborted,
  createClient,
} from '@chronoai/cma-sdk';

const cma = createClient();

try {
  await cma.steer('sesn_example', 'Change direction');
} catch (error) {
  if (error instanceof CapabilityRefused) {
    console.log('not granted', error.capability);
  } else if (error instanceof DeploymentAtCapacity) {
    console.log(error.freeCpuMillis, error.freeMemoryMib);
  } else if (error instanceof RequestAborted) {
    console.log('cancelled by caller');
  } else if (error instanceof ApiError) {
    console.log(error.status, error.code, error.requestId, error.message);
  } else {
    throw error;
  }
}
```

| Error                   | Meaning                                                   | Retry decision                                            |
| ----------------------- | --------------------------------------------------------- | --------------------------------------------------------- |
| `CapabilityRefused`     | This session permanently lacks the named capability       | Never for this session                                    |
| `SessionLimitReached`   | The owner already holds the live-session limit            | Stop a session or obtain the explicit exemption           |
| `DeploymentAtCapacity`  | Declared free CPU or memory is below the request          | Stop a session or request a smaller size                  |
| `ApiError`              | Structured non-success response                           | Inspect `status`, `code`, and `isRetryable`               |
| `ValidationError`       | Caller input cannot be represented                        | Fix the request                                           |
| `CredentialError`       | Credential callback failed or returned an invalid value   | Repair or refresh credential state                        |
| `RequestIdError`        | Request-id provider failed or returned an invalid value   | Repair the provider                                       |
| `ResponseObserverError` | Response observer threw or rejected                       | Repair the observer; the response was not otherwise read  |
| `RequestAborted`        | The caller cancelled the operation                        | Retry only if the caller still wants it                   |
| `TimeoutError`          | The configured request deadline elapsed                   | Outcome may be unknown                                    |
| `TransportError`        | No valid HTTP answer was obtained                         | A later attempt may work                                  |
| `ProtocolError`         | A success, problem, or stream frame violated the contract | Do not retry blindly; investigate version or server drift |

`ApiError` and its typed refusal subclasses expose the response's correlation id
as `requestId`. That value comes only from the HTTP `X-Request-Id` response header,
never from a `request_id` member in a problem body. A missing, blank, malformed, or
otherwise unusable response header produces `undefined`; the SDK does not invent a
response id or substitute the caller's request id.

A configured `responseObserver` runs for every received HTTP response before the
SDK makes a retry decision or decodes the status or body. If the observer throws or
rejects, the operation stops with `ResponseObserverError`, whose `cause` preserves
the observer failure.

## Retry discipline

Unary TypeScript operations do not retry automatically. `ApiError.isRetryable` is
true for `429`, `5xx`, and the self-resolving `409 session_rotating` refusal.
Other `409` responses, including `session_terminated`, are not retryable.
`TransportError.isRetryable` is also true, but these values are advice to the caller
rather than an automatic replay. A caller may retry `session_rotating` after the
in-progress plane change finishes. For `POST /sessions/{id}/input`, a completed
refusal remains bound to its `Idempotency-Key` for 24 hours; retry the intended
input with a fresh key after readiness, because reusing the refused key replays
the original `409`. AG-UI applies the same rule to an explicit `runId`. The SDK
does not make either retry automatically.

The durable event stream performs its own bounded reconnects as documented in
[Streaming](streaming.md). A server problem document that does not match the
[REST error contract](../../../docs/API_REST.md#10-errors) becomes `ProtocolError`
rather than an unreliable generic refusal.
