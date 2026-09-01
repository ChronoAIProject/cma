# Rust Sessions

<!-- nav: 40 Sessions -->

A session is the durable record CMA keeps while runtimes are provisioned, released,
lost, or rotated underneath it. Earlier `Session` values do not update in place;
use `read_session` whenever the current lifecycle or metadata matters.

## Create, list, and read

```rust,no_run
use cma_sdk::{Client, CreateSessionRequest, TerminalGeometry};

async fn inspect(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let request = CreateSessionRequest {
        surface: None,
        geometry: Some(TerminalGeometry::new(120, 32)?),
        environment_profile: Some(String::from("profile_backend")),
        quoted_rate_card: None,
    };
    let created = client.create_session(request).await?;
    let all = client.list_sessions().await?;
    let current = client.read_session(&created.id).await?;

    println!("{} {}", all.len(), current.id);
    Ok(())
}
```

`list_sessions` follows the server's opaque `Link: rel="next"` continuations and
returns only after every visible page has been validated. The compatibility walk
refuses more than 100 pages or 20,000 retained rows by default. Use
`list_sessions_with_limits` with `PageWalkLimits` when a smaller resource ceiling
fits the caller.

Use `list_session_page` when the caller should decide when to fetch more. A
continuation is route-bound and opaque; pass it to `SessionPageRequest::continuing`
without parsing it:

```rust,no_run
use cma_sdk::{Client, SessionPageRequest, SessionStatus};

async fn running_page(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let request = SessionPageRequest::new()
        .with_status(SessionStatus::Running)?
        .with_limit(50)?;
    let first = client.list_session_page(request).await?;
    println!("{}", first.items.len());

    if let Some(next) = first.next {
        let second = client
            .list_session_page(SessionPageRequest::continuing(next))
            .await?;
        println!("{}", second.items.len());
    }
    Ok(())
}
```

First-page limits are 1 through 200. `with_cursor` carries an opaque cursor
received out of band; a server-issued continuation already contains the original
status and limit and therefore cannot be combined with new filters.

`quoted_rate_card` accepts a sealed identifier obtained outside this crate. This
version does not expose the quote endpoint, environment-profile CRUD, session stop,
revive, terminate, delete, or plane-change operations. Their absence is explicit;
do not build unvalidated JSON around the client to imitate them.

## Lifecycle and recovery

`SessionStatus` names `Provisioning`, `Ready`, `Running`, `Rotating`, `Idle`, and
`Terminated`. `RecoveryState` is separate operational health. While status is
`Rotating`, the server refuses new input with the retryable `409 session_rotating`
error. Clients should disable input controls for experience and keep reading events
until the replacement runtime is usable; correctness does not depend on that client
guard. `StopReason::RequiresAction` means the runtime is waiting for an owner
decision and is not terminal.

The Rust session enums currently validate known wire values strictly. A future
session lifecycle or recovery word therefore becomes `Error::Protocol` until a new
crate version names it. Event discriminators have a different forward-compatibility
rule and are preserved as `EventKind::Unknown`; see [Streaming](streaming.md).

## Conformance classes

`Session::conforms_to` is the server-granted capability set. Check it rather than
inferring behavior from the kernel name:

```rust,no_run
use cma_sdk::Session;

fn may_steer(session: &Session) -> bool {
    session
        .conforms_to
        .iter()
        .any(|class| class.ends_with("/conf/steering"))
}
```

The server remains authoritative under a race and returns
`Error::CapabilityRefused { capability, request_id }` when an operation was never
granted. `request_id` is the optional `X-Request-Id` response header used for
correlation.
The complete lifecycle and capability model is in the
[API overview](../../../docs/API_Overview.md#2-session-model).
