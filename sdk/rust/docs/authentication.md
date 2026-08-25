# Rust Authentication

<!-- nav: 30 Authentication -->

The Rust SDK does not use browser cookies and does not mint or persist bearer
credentials. A client without a credential can call the anonymous
`read_conformance` discovery operation. Authenticated operations send no
`Authorization` header until a provider is configured, so the service normally
answers those operations with `401`.

```rust,no_run
use cma_sdk::Client;

async fn discover() -> cma_sdk::Result<()> {
    let client = Client::builder("https://cma.example.com")?.build()?;
    let deployment = client.read_conformance().await?;
    println!("{} at {}", deployment.version, deployment.build_sha);
    Ok(())
}
```

How an authenticated credential is issued is deployment-specific; when there is
no self-service path, obtain one from the deployment's operator or identity
administrator.

Supply a callback rather than a fixed header:

```rust,no_run
use cma_sdk::Client;

fn client() -> Result<Client, Box<dyn std::error::Error>> {
    let client = Client::builder("https://cma.example.com")?
        .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
        .build()?;
    Ok(client)
}
```

The provider runs for every request attempt and event-stream reconnect, so an
external secret store can return its newest credential after rotation. An empty
value is a protocol error; a provider failure is a transport error before the HTTP
request is sent. `Debug` for `ClientBuilder` and `Client` never renders the callback
or anything it captured, and the Authorization header is marked sensitive.

```rust,no_run
use std::sync::{Arc, RwLock};

use cma_sdk::Client;

fn rotating_client() -> Result<Client, Box<dyn std::error::Error>> {
    let credential = Arc::new(RwLock::new(String::from("set-by-secret-store")));
    let current = Arc::clone(&credential);
    let client = Client::builder("https://cma.example.com")?
        .credential_provider(move || current.read().map(|value| value.clone()).map_err(|_| ()))
        .build()?;
    Ok(client)
}
```

## Request correlation

Use `request_id_provider` to supply an `X-Request-Id` for each logical HTTP
operation. The provider runs once before the retry loop, and the SDK sends its
exact value unchanged on every attempt. This makes a caller trace or support ID
stable even when the credential provider rotates between attempts.

`response_observer` receives the status and usable response `X-Request-Id` before
the SDK inspects retry policy or decodes a success or refusal body. It therefore
also sees completed intermediate `429` and `503` responses that are retried.

```rust,no_run
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};

use cma_sdk::Client;

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn observable_client() -> cma_sdk::Result<Client> {
    Client::builder("https://cma.example.com")?
        .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
        .request_id_provider(|| {
            let sequence = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(format!("worker-a-{sequence}"))
        })
        .response_observer(|metadata| {
            eprintln!(
                "CMA status={} request_id={:?}",
                metadata.status, metadata.request_id
            );
            Ok::<_, Infallible>(())
        })
        .build()
}
```

Provider values must be non-blank and valid HTTP header text. Missing, blank,
duplicated, or non-visible response values appear as `None`; they do not damage
an otherwise valid response. Observer errors and panics stop handling explicitly
as `Error::Observer` before the response body is consumed.

A contract-conforming HTTP `401` is returned as
`Error::Api { status: 401, .. }`. A nonconforming unauthenticated response, including
one without the required problem code, becomes `Error::Protocol`; the current
service has an open error-contract gap on that path, so handle both until the
deployment demonstrates the normalized problem shape. `Error::CapabilityRefused`
is different: the caller was identified, but this session permanently lacks the
named operation, so another attempt cannot change the answer. See
[Errors](errors.md) and the
[REST authentication contract](../../../docs/API_REST.md#authentication).
