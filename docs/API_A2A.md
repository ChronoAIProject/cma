# A2A Interface

A2A is not implemented in this release. `GET /.well-known/agent-card.json` and
requests to `/api/v1/a2a` return `501 application/problem+json` with
`code: "interface_not_available"`, `interface: "a2a"` and
`Cache-Control: no-store`.

There is no served Agent Card, task, task subscription, push-notification
configuration or A2A message protocol. The interface declares no core
operations. The discovery refusal is deliberate; clients must not interpret
it as an empty usable card or a temporary failure that starts provisioning.

CMA Agent identifiers currently belong to the [REST](API_REST.md),
[AG-UI](API_AGUI.md) and [Responses](API_Responses.md) surfaces. A future A2A
projection must preserve the same ownership and durable conversation semantics;
this document does not promise a speculative task-to-Agent mapping.
