# Rust Errors

<!-- nav: 70 Errors -->

All operations return `cma_sdk::Result<T>`. `Error` is non-exhaustive so consumers
must retain a fallback arm when matching it.

```rust,no_run
use cma_sdk::{Client, Error, SessionId};

async fn interrupt(client: &Client, session_id: &SessionId) -> cma_sdk::Result<()> {
    match client.interrupt(session_id).await {
        Ok(()) => Ok(()),
        Err(Error::CapabilityRefused {
            capability,
            request_id,
            ..
        }) => {
            eprintln!("session was not granted {capability}; request_id={request_id:?}");
            Ok(())
        }
        Err(Error::Api {
            status,
            code,
            message,
            request_id,
            ..
        }) => {
            eprintln!("HTTP {status} {code}: {message}; request_id={request_id:?}");
            Ok(())
        }
        Err(error) => Err(error),
    }
}
```

| Variant | Meaning | Retry decision |
|---|---|---|
| `CapabilityRefused` | This session permanently lacks the named capability | Never for this session |
| `Api` | Structured service problem with HTTP status, code, and detail | Only according to status and operation semantics |
| `Transport` | Credential provider, timeout, connection, or response-body failure | A later attempt may work; outcome can be unknown |
| `Protocol` | Caller value or server response violated the typed boundary | Fix input or investigate version/server drift |
| `Observer` | The configured response metadata observer failed or panicked | Repair the observer; the response body was not consumed |

The crate retries transport failures according to `RetryPolicy`. It retries HTTP
`429` or `503` only when the response carries a valid `Retry-After`; an API failure
without that header is returned immediately. Other API and protocol failures are
not retried.

An exact `Error::Api { status: 409, code: "session_rotating", .. }` is a
self-resolving lifecycle refusal. The SDK returns it instead of automatically
replaying the operation. Re-read the session before a later attempt, inspect its
current status, and only then decide whether to retry according to the operation's
side-effect and idempotency semantics. A repeated `session_rotating` refusal means
the handover still fences interaction even if the stored status has already
advanced. If the refused input carried a caller-supplied `Idempotency-Key`, use a
fresh key for the post-rotation attempt: the original key stores and replays its
`409` response.

The REST service accepts optional idempotency keys for session creation and input,
but this SDK's high-level methods do not yet expose or automatically supply them. A
transport failure after a create or input may therefore hide a completed server
action. A caller that cannot tolerate automatic replay can use a one-attempt policy
and read current state before deciding what to repeat:

```rust,no_run
use std::time::Duration;

use cma_sdk::{Client, RetryPolicy};

fn conservative_client() -> cma_sdk::Result<Client> {
    let retries = RetryPolicy::new(1, Duration::ZERO, Duration::ZERO)?;
    Client::builder("https://cma.example.com")?
        .credential_provider(|| Ok::<String, ()>(String::from("set-by-secret-store")))
        .retry_policy(retries)
        .build()
}
```

Problem documents that do not match the
[REST error contract](../../../docs/API_REST.md#11-errors) become `Error::Protocol`
rather than an unreliable generic API error.

`Error::Api` and `Error::CapabilityRefused` expose `request_id` from the HTTP
`X-Request-Id` response header. It is `None` when that header is missing or
unusable. Correlation is not read from, or represented as, a `request_id` field
inside the problem JSON body.
