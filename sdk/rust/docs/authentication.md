# Authentication

<!-- nav: 3 Authentication -->

Configure resource credentials through `ClientBuilder::bearer_token`:

```rust,no_run
use cma_sdk::Client;

# fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder(std::env::var("CMA_BASE_URL")?)?
    .bearer_token(std::env::var("CMA_BEARER_TOKEN")?)?
    .build()?;
# Ok(())
# }
```

The SDK does not obtain, refresh, or persist credentials. All resource calls use
the configured token; discovery always omits it, even on an authenticated
client. A client without a token can access public discovery and marketplace
reads, while CMA enforces authorization on owner resources.

Do not place a credential in the deployment URL. `Client::builder` rejects URLs
containing user information so credentials cannot leak through debug output,
logs, or proxy request targets.

Bearer headers are marked sensitive, and `Debug` output only reports whether
authentication is configured. Redirects are not followed. Use HTTPS outside
localhost so transport encryption protects credentials. Do not place tokens in
profile environment variables, checked-in examples, logs, or error messages.
