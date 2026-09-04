# Quickstart

<!-- nav: 2 Quickstart -->

Build one client for a deployment root and read its anonymous discovery
document:

```rust,no_run
use std::time::Duration;
use cma_sdk::Client;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com/")?
    .request_timeout(Duration::from_secs(5))?
    .build()?;
let deployment = client.read_conformance().await?;
assert_eq!(deployment.service, "cma");
# Ok(())
# }
```

A trailing slash is normalised. Deployment URLs must use HTTP or HTTPS and may
not contain user information, a query, or a fragment.
