# CMA Rust SDK

<!-- nav: 10 Overview -->

`cma-sdk` is the asynchronous backend client for CMA's REST interface. It is a
standalone Cargo project, uses Tokio and rustls, and has no dependency on CMA's
service crates.

## Use from the repository

This crate is repository-only, carries `publish = false`, and is not published to
crates.io. Add it by path from a checkout:

```sh
cargo add cma-sdk --path /path/to/cma/sdk/rust
cargo add tokio --features macros,rt-multi-thread
```

The in-tree version remains `0.0.0` because no registry release is produced. CI
installs the crate by path into a fresh consumer, which proves the examples use
only its exported surface.

## Guides

The navigation comment immediately after each H1 is build metadata for CMA's
Developer surface. Its form is `<!-- nav: <order> <label> -->`. These same files
are included under `cma_sdk::guides`, so every Rust fence is a cargo doctest.

| Guide | Use it for |
|---|---|
| [Quickstart](quickstart.md) | Create a session and consume its stream |
| [Authentication](authentication.md) | Bearer providers and credential rotation |
| [Sessions](sessions.md) | Creation, lifecycle, conformance, and SDK scope |
| [Streaming](streaming.md) | Cursors, reconnects, events, and cancellation |
| [Triggers](triggers.md) | Publish repository Triggers and run contribution Launches |
| [Operations](operations.md) | Input, approvals, control, terminal REST, and usage |
| [Errors](errors.md) | Typed failures and retry decisions |
| [Reference](reference.md) | Locally generated rustdoc and public exports |

## Scope

This crate exposes anonymous deployment and Trigger discovery, publisher Trigger
management, NyxID GitHub connection handoff, the complete contributor Launch and
pull-request workflow, Responses chat conversations and turns, session
create/list/read, durable event streaming, input, approval resolution, interrupt,
steer, terminal resize and screen snapshot, and usage. It deliberately does not
expose the live bidirectional PTY WebSocket, environment-profile administration,
pricing, resources, or session stop/revive/delete operations. An absent method is
an absence, not an invitation to assume a wire shape.

These guides explain how to use the crate. The authoritative wire documents are
the [REST contract](../../../docs/API_REST.md),
[Responses contract](../../../docs/API_Responses.md),
[API overview](../../../docs/API_Overview.md), and
[PTY contract](../../../docs/API_PTY.md).
