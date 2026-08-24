# CMA SDKs

CMA's integration documentation lives with the repository-only SDKs:

- [TypeScript SDK guide](../sdk/typescript/docs/README.md)
- [Rust SDK guide](../sdk/rust/docs/README.md)

Neither SDK is published to an external package registry. The TypeScript package
is private and consumed locally by `web/`; the Rust crate is `publish = false` and
consumed by path. Local package and scratch-consumer checks keep both exported
surfaces usable.

Those files are versioned beside their SDKs and are the designated source for the
CMA Developer surface. The public wire contracts remain in the interface documents:
[REST](API_REST.md), [AG-UI](API_AGUI.md), [PTY](API_PTY.md), [MCP](API_MCP.md),
and [A2A](API_A2A.md).
