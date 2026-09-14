# MCP Interface

MCP is not implemented in this release. `POST /api/v1/mcp` returns
`501 application/problem+json` with `code: "interface_not_available"`,
`interface: "mcp"` and `Cache-Control: no-store`.

The interface declares no core operations. It does not advertise tools,
resources, prompts, JSON-RPC initialization, an Agent creation method or a
conversation identifier mapping. A protocol client must treat the refusal as
unsupported, not retry initialization indefinitely.

Use [REST](API_REST.md) for Sandbox, Workspace, Worktree, Agent, profile and
Trigger resources, and [AG-UI](API_AGUI.md) or [Responses](API_Responses.md) for
Agent conversations. Future MCP support must project this same model and prove
its supported operations before the declaration changes.
