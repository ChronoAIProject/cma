# Provider Discovery And A2A

`GET /.well-known/agent-card.json` returns public CMA provider discovery with
HTTP 200 and `Cache-Control: no-store`. It requires no login and ignores ambient
cookies or bearer credentials, including when the identity store is unavailable.
Its `url` comes from the existing `CHRONO_SERVER_PUBLIC_URL` configuration.

```json
{
  "name": "CMA Codex provider",
  "version": "<crate version>",
  "url": "<configured public base>",
  "capabilities": {
    "streaming": true,
    "activity_items": true,
    "client_tools": false,
    "cancel": true,
    "rich_input": false
  },
  "provider_contract": {
    "binding": "/api/v1/cmaeg/bindings/{binding_id}",
    "conversation": "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
    "responses": "/api/v2/responses"
  },
  "authentication": { "schemes": ["bearer-agent-key"] }
}
```

Binding turns use the durable [Responses v2 contract](API_Responses.md). The
verified subset includes conversation aliases, informational `mcp_call` activity,
immutable event-context retrieval, and explicit cancellation of queued responses
or exact bound turns through the documented CMA control routes. `cancel: true`
refers to those controls; the standard OpenAI cancellation endpoint is still
unsupported. Client-executed tools and rich input remain unavailable. Binding
keys cannot resolve approvals; the owner's dashboard retains that authority.

The gateway's normative classifier selects the highest advertised recognized
capability. Consequently this card classifies as level 4 because `cancel` is true;
CMA's binding response records its level-2 turn contract. These fields describe
different contracts and do not change the classifier or imply support for
client-executed tools. Local HTTP and worker fixtures prove the implemented
behavior; they are not production-provider or Telegram delivery evidence.

A2A JSON-RPC is unavailable. Every request to `/api/v1/a2a` returns
`501 application/problem+json` with `code: "interface_not_available"`,
`interface: "a2a"` and `Cache-Control: no-store`. There are no A2A tasks,
subscriptions, push-notification settings or message operations. The provider
card directs clients to Responses; it does not advertise A2A message support.
