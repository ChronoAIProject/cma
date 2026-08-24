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

`quoted_rate_card` accepts a sealed identifier obtained outside this crate. This
version does not expose the quote endpoint, environment-profile CRUD, session stop,
revive, terminate, delete, or plane-change operations. Their absence is explicit;
do not build unvalidated JSON around the client to imitate them.

## Lifecycle and recovery

`SessionStatus` names `Provisioning`, `Ready`, `Running`, `Rotating`, `Idle`, and
`Terminated`. `RecoveryState` is separate operational health. While status is
`Rotating`, refuse new input and keep reading events until the replacement runtime
is usable. `StopReason::RequiresAction` means the runtime is waiting for an owner
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
`Error::CapabilityRefused { capability }` when an operation was never granted.
The complete lifecycle and capability model is in the
[API overview](../../../docs/API_Overview.md#2-session-model).
