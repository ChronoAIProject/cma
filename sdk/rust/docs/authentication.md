# Rust Authentication

<!-- nav: 30 Authentication -->

The Rust SDK is a backend bearer client. It does not use browser cookies and does
not mint or persist credentials. How a credential is issued is deployment-specific;
when there is no self-service path, obtain one from the deployment's operator or
identity administrator.

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

A contract-conforming HTTP `401` is returned as
`Error::Api { status: 401, .. }`. A nonconforming unauthenticated response, including
one without the required problem code, becomes `Error::Protocol`; the current
service has an open error-contract gap on that path, so handle both until the
deployment demonstrates the normalized problem shape. `Error::CapabilityRefused`
is different: the caller was identified, but this session permanently lacks the
named operation, so another attempt cannot change the answer. See
[Errors](errors.md) and the
[REST authentication contract](../../../docs/API_REST.md#authentication).
