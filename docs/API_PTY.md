# PTY Interface

PTY is absent in the Sandbox/Agent release. There is no terminal plane,
attachment ticket, terminal resize, terminal snapshot, WebSocket attachment or
plane-change operation. Clients must not offer a terminal selector or infer
one from an Agent provider's separate command-line interface.

`GET /api/v1/agents/{id}/terminal` first checks ownership. A missing or foreign
Agent returns `404`; an owned Agent returns `501 application/problem+json` with
`code: "capability_unsupported"` and `capability: "terminal"`.

This explicit refusal is not a transient deployment failure and should not be
retried as a connection attempt. No runtime is started by the request. Agent
conversations use [AG-UI](API_AGUI.md), [Responses](API_Responses.md), or the
documented [REST](API_REST.md) operations.
