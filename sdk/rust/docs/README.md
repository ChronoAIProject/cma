# CMA Rust SDK

<!-- nav: 1 Overview -->

The temporary Rust SDK reads the public state of a CMA deployment while its
resource interfaces are rebuilt. It deliberately carries no retired or
speculative resource model.

```rust,no_run
use cma_sdk::Client;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com")?.build()?;
let deployment = client.read_conformance().await?;
println!("{}: {}", deployment.service, deployment.status);
# Ok(())
# }
```
