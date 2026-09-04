# Errors

<!-- nav: 4 Errors -->

The client returns `cma_sdk::Error`. `Error::Api` represents a structured
non-success response, `Error::Protocol` rejects an invalid response boundary,
and `Error::Transport` covers failures before a complete HTTP answer is read.

```rust,no_run
use cma_sdk::{Client, Error};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com")?.build()?;
match client.read_conformance().await {
    Ok(document) => println!("{}", document.status),
    Err(Error::Api { status, code, .. }) => eprintln!("{status}: {code:?}"),
    Err(error) => return Err(error.into()),
}
# Ok(())
# }
```
