# Errors

<!-- nav: 10 Errors -->

The client returns `cma_sdk::Error`. `Error::Api` represents a structured
non-success response, `Error::Protocol` rejects an invalid response boundary,
`Error::Transport` covers failures before a complete HTTP answer is read, and
`Error::Validation` rejects invalid caller input before sending it.

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

`Error::Api` preserves HTTP `status`, stable `code`, the safe problem `message`,
`request_id`, validated `retry_after`, and structured `extensions`. Retry delays
support positive seconds and future HTTP dates. Missing, malformed, zero, or
past delays are ignored. Use `extension("field")`, `extension("path")`, or
`extension("reason")` for string-valued hints; inspect `extensions` directly for
structured limits and counts. Do not infer machine behavior by parsing prose.

`is_unauthenticated()` identifies HTTP 401. `is_retryable()` identifies transport
failures, HTTP 429, and HTTP 5xx; it is a classification, not a retry policy.
The SDK performs no automatic retries. A timed-out mutation may already have
been accepted: retry only with its original key and unchanged body, and do not
assume that a new key is safe. Authorization, trust, conflict, and validation
refusals require the caller to address their cause.

Malformed problem documents, a problem status that contradicts the HTTP status,
oversized responses, and unexpected successful status codes are protocol errors
rather than fabricated resource values. Transport failures omit request URLs
and credentials from their messages.

For streams, invalid UTF-8, JSON, closed tags, event names, or cursor ordering
produce protocol errors and stop the reader. Interrupted transport remains a
transport error. Responses EOF before a terminal event is ambiguous: retrieve
the known response or durable Agent history before considering another input.
AG-UI and Launch replay can reconnect explicitly from the last fully consumed
durable cursor; Responses sequence numbers cannot be used that way.
