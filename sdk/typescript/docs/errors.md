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
    console.log(error.status, error.code, error.message);
  } else {
    throw error;
  }
}
```

| Error                  | Meaning                                                   | Retry decision                                            |
| ---------------------- | --------------------------------------------------------- | --------------------------------------------------------- |
| `CapabilityRefused`    | This session permanently lacks the named capability       | Never for this session                                    |
| `SessionLimitReached`  | The owner already holds the live-session limit            | Stop a session or obtain the explicit exemption           |
| `DeploymentAtCapacity` | Declared free CPU or memory is below the request          | Stop a session or request a smaller size                  |
| `ApiError`             | Structured non-success response                           | Inspect `status`, `code`, and `isRetryable`               |
| `ValidationError`      | Caller input cannot be represented                        | Fix the request                                           |
| `CredentialError`      | Credential callback failed or returned an invalid value   | Repair or refresh credential state                        |
| `RequestAborted`       | The caller cancelled the operation                        | Retry only if the caller still wants it                   |
| `TimeoutError`         | The configured request deadline elapsed                   | Outcome may be unknown                                    |
| `TransportError`       | No valid HTTP answer was obtained                         | A later attempt may work                                  |
| `ProtocolError`        | A success, problem, or stream frame violated the contract | Do not retry blindly; investigate version or server drift |

## Retry discipline

Unary TypeScript operations do not retry automatically. `ApiError.isRetryable` is
true for `429` and `5xx`, and `TransportError.isRetryable` is true, but those are
advice to the caller rather than an automatic replay. This matters for
side-effecting POSTs: the current REST contract has no idempotency key, so a timeout
can mean the server completed an operation whose response was lost. Read current
state before repeating create or another non-idempotent operation.

The durable event stream performs its own bounded reconnects as documented in
[Streaming](streaming.md). A server problem document that does not match the
[REST error contract](../../../docs/API_REST.md#9-errors) becomes `ProtocolError`
rather than an unreliable generic refusal.
