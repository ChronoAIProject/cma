# CMA Bot Father: agent ownership and user sessions

This document describes CMA Bot Father and its ownership boundaries. The
delivery section distinguishes implemented behavior from production evidence. Public gateway contracts remain in the gateway's own
[integration guide](CMAEG_Integration.md) and [protocol](CMAEG_Protocol.md).

## Responsibility

CMA Bot Father is a normal CMA assistant in an always-live, ChronoAI-owned
Sandbox. Its instructions and skills provide knowledge about CMA, general
assistance, delegated CMA resource operations, and event-channel creation.
Ordinary messages always reach the agent. The agent decides when to offer an
operation, ask for missing choices, or continue an earlier task.

CMAEG authenticates and normalizes source events, forwards the configured target
and verified source metadata to the provider, and delivers replies and generic
interactions. The provider selects the actual agent and owns user affinity. CMAEG does not
interpret `/setup`, decide whether a message requests a Bot, run a CMA resource setup state
machine, manage user login, or retain end-user NyxID delegation grants.

CMA retains linked user identities, encrypted credential custody, revocation,
pending actions and authentication callbacks. The agent receives status and
operation results; the underlying NyxID access/refresh credentials remain in
CMA's credential broker. Never ask a Telegram user to paste credentials into chat.

The Father's organization credential runs the assistant. Each user's authenticated
delegation authorizes that user's resource operations. These authorities must not
be interchangeable. Organization operations additionally recheck membership and
use the chosen organization's service inventory, connections and agent-key scope.

## Channel targets and subconnections

A channel declares a provider and a target. Agent selection belongs to that
provider in both modes:

| Target | Conversation behavior | Resource lifecycle |
| --- | --- | --- |
| Existing agent instance | The gateway forwards the specified target. The provider sends every admitted event to that agent, regardless of sender. | Attach to the authorized existing instance. Deleting the channel detaches it; it must not delete the borrowed agent, workspace or sandbox. |
| Event Worker Profile | The gateway forwards the profile target and verified event identity. The provider applies its own session policy, reuses the appropriate agent, or creates a new instance. | The provider owns user affinity, instance lookup, provisioning and cleanup of the instances it created. |

The channel's `provider.target` accepts `{"type":"profile"}` (the default) or
`{"type":"existing_agent","agent_id":"agt_..."}`. CMA validates the existing
agent's owner and live workspace/sandbox before attaching it. That target is
immutable. Existing-agent attachment supports automatic final replies (L1);
it does not replace the attached agent's credential or inject a second channel's
native tools. Providers reject unsupported targets explicitly.

The gateway preserves authenticated platform/sender, source chat/thread, event ID
and reply destination as event metadata. It does not hash the sender into an
agent-selection key, maintain a user-to-agent map, or decide whether an agent
needs creating. A verified Telegram sender is a source identity; it is not proof
of a NyxID account. CMA owns the authenticated account association after login.

A subconnection is a provider-owned session beneath one channel binding. CMA's
profile extension `environment.session_affinity: "source_user"` derives its
session from the binding generation, verified source platform and human sender.
The same sender across chats reuses its Agent, Worktree and conversation; different
senders never share them. CMA attests the original Activity through the gateway
before choosing or provisioning an agent. NyxID delegation belongs to that source
actor and is established separately through explicit consent. CMA does not merge
separate source identities merely because they later sign into the same account.
Each event retains its own reply destination regardless of the selected agent.
The original transport chat and the provider session are distinct identifiers.
Transport conversation creation/deletion acknowledges the transport lifecycle
without creating or retiring user agents; binding retirement cleans them up.

Repeated events, retries, gateway restarts and simultaneous first events must
resolve to the same provider-selected instance. This guarantee must be enforced
by CMA's durable mapping and admission logic, not by a gateway sender partition.
The existing gateway `partition` fields are compatibility behavior of the current
protocol; adding a new gateway `sender` rule does not implement this requirement.
The profile-target provider contract must let the provider choose the session
without treating a gateway-derived conversation ID as the user-affinity authority.

The separate `environment.verified_sender_context:true` input extension is for
existing-Agent targets only; profile targets refuse it by name. It does not
change source-user affinity. See [CMA binding extensions](API_REST.md).

Bot Father uses the profile target. CMA retains one always-live Sandbox and
Workspace, with a distinct Agent, Worktree, model conversation and per-user
operation context for each subconnection. Different agents may run concurrently;
turns for one agent remain ordered.

The current provider projection caps a workspace at 16 agents. Capacity must be
explicit, observable and enforced without mapping new users onto someone else's
agent. Exhaustion needs a clear retry/capacity result; increasing capacity requires
validating the existing sandbox's resources and continuity constraints.

## User login and automatic continuation

General questions do not need login. When an operation needs user authority:

1. The agent asks CMA for a sign-in link tied to the current verified actor,
   binding generation and provider conversation. CMA retains the pending intent
   and exact agent destination before returning the link.
2. The user completes NyxID sign-in in the browser. OAuth state, PKCE, expiration
   and the callback remain CMA-owned. Browser authentication identifies the
   NyxID account; it does not by itself prove ownership of a Telegram sender.
3. CMA activates an explicit delegation to the initiating Telegram actor. A
   verified Telegram identity matching that actor can prove the association.
   Alternatively, a CMA browser consent flow must explicitly authorize the
   displayed Telegram actor using the authenticated account. That is an explicit
   delegation, not an assertion that the two accounts have the same identity.
   A login callback GET alone must not silently activate authority.
4. Activation durably records an authentication-completed notification. CMA
   delivers it to the exact original agent conversation, including after an API
   restart or a busy agent. The notification carries grant identity/status, never
   credentials, and uses an idempotent command ID.
5. The agent automatically continues the pending conversation and asks for the
   remaining choices. The user need not type a code, slash command or another
   message merely to make the agent notice sign-in.

Authentication completion is a system notification, not a human instruction to
create resources. The agent must preserve the requested operation and obtain any
required confirmation of the effective owner and resource configuration.

Callback delivery must recheck the current link, revocation, credential
incarnation, binding generation and agent mapping. An expired or superseded login
cannot wake another user's agent or grant authority to a replacement conversation.
Retries must not duplicate the operation or start a second agent.

The browser flow uses a fresh, browser-bound PKCE login and displays the exact
Telegram actor before consent. A callback GET cannot activate delegation. The
explicit consent POST atomically activates the grant and appends its durable
wakeup. CMA leases pending wakeups, admits one deterministic agent command, and
rechecks authority while waiting for completion. Lost responses, API restarts and
lease expiry reuse the admitted command and delivery identity. A revoked,
replaced or expired grant cannot continue the operation.

The callback gives the agent a CMA continuation reference, valid only for CMA's
delegated and setup APIs. It resolves to the original signed event and timestamp;
it is not a new user instruction or a gateway reply capability. CMA retains the
24-hour reply target before returning a login link and sends the callback result
to the original source chat. Credentials never enter the agent prompt.

## Connecting an event channel

CMA works with opaque gateway connection references. The agent asks CMA for a
connection setup operation using the current verified event. CMA rechecks that
event's linked user grant, incarnation, selected personal or organization owner,
and binding credential, then calls the gateway on behalf of that owner. A durable
operation UUID and the original event reference make retries recoverable.

The gateway returns its own setup URL. All platform selection, credential entry,
creation methods and optional platform capabilities are presented there. The
provider receives generic status, a display label and an opaque connection
reference; platform credentials never enter CMA or the agent conversation.
NyxID user credentials remain in CMA's delegation broker and are used transiently
for authenticated gateway requests. There is no NyxBot creation dependency.

Gateway completion is a verified event notification to the original agent and
conversation. CMA refreshes the pending operation and continues the access-rule
and review steps. This notification is not fresh human consent: creating CMA
resources still requires the subsequent reviewed-plan confirmation. On
confirmation CMA binds its provider profile or agent to the returned connection.
New connection-based Bots use only the agent credential; retained legacy Bots
keep their existing transport records and lifecycle.

The gateway's public [protocol](CMAEG_Protocol.md) owns the platform setup contract.

## Delegated operations and isolation

The binding-authenticated `POST /api/v1/agent-delegations/identity` resolves
sign-in status or a login URL from the current event. The companion
`POST /api/v1/agent-delegations/operations` dispatches allowlisted ordinary CMA
resource operations as that user. It uses existing resource services and command
admission, checks current grant and membership, and retains delegation attribution
through deferred execution. The model cannot supply a user token or grant ID.
Mutations require an idempotency key. Bot creation uses the typed
`POST /api/v1/bot-setups/actions` workflow to retain review and final consent.
See [REST](API_REST.md#agent-delegated-cma-operations) for request examples.

The credential broker keeps user secrets outside the shared sandbox. Per-user
history, draft configuration and tool results must never be added to another
agent's prompt. Shared skills and CMA documentation may be reused. User-specific
memory and grants may not be shared.

Bot Father enables CMA's `environment.isolated_agents: true` provider extension.
Each agent process runs in private filesystem, process and network namespaces.
Only its own home/worktree is writable; sibling histories, shared runtime state,
shared credentials and host runtime control are inaccessible. Shared skills and
tools are read-only. Public HTTPS research passes through a proxy that rejects
private destinations. A constrained platform relay permits only model/research
services and the event-authorized CMA delegation/setup APIs; it cannot use the
Father's organization credential to list siblings or manage resources directly.

This is an opt-in runtime launch policy for Bot Father agents. It does not rotate
retained sandboxes or change other users' agent profiles. Resource ownership and
grant checks still run in CMA for every delegated operation.

## Callback replies and gateway boundaries

All provider agent instances use the same gateway reply API. The gateway does
not select a recipient from an agent ID or a channel's most recent sender.
Existing `POST /v1/events/{event_ref}/replies` / `replyToEvent` uses the sealed
event reference to recover its original relay message and route. NyxID resolves
that original message to its source conversation. An automatic final response
likewise stays attached to the event that opened the provider stream.

Agent selection and reply routing are independent. For example, user A's event
and reply reference are dispatched by CMA to Agent A, while user B's event and
reply reference are dispatched to Agent B. Either agent may finish first; each
reply still uses its own event context. A provider may instead choose one shared
agent without changing this delivery rule. One agent may handle multiple source
chats; its identity alone must never determine where a response is sent.

A callback can arrive after its initiating turn completed. CMA therefore needs a
generic way to resume its provider conversation and send a result to the retained
source destination. Current short-lived event references are not indefinite
conversation credentials. Do not disable expiry to make delayed callbacks work.

The generic gateway extension mints an opaque `reply_target_ref` through
`POST /v1/events/{event_ref}/reply-target`. It authenticates the retained original
Activity and scopes the target to that original message, channel, binding and
current channel key for 24 hours. It contains no user login grant or instance
selection policy.

`POST /v1/reply-targets/{reply_target_ref}/messages` accepts the normal outbound
message and a required `Idempotency-Key`. Every agent/provider uses the same
endpoint; the authenticated target determines the destination. Both mint and
send recheck uncached current channel authority. Expired, deleted or rekeyed
channels fail closed. Claims and receipts are durably encrypted in the generic
interaction store, so a gateway restart can replay a confirmed receipt. A changed
payload under the same key conflicts. An unknown platform send is not retried
automatically or described as confirmed delivery. The existing four native MCP
tool names remain unchanged; delayed delivery is a provider REST capability.

CMA retains the originating reply target with the pending login/operation and
durable wakeup. It does not substitute whichever chat most recently contacted
the agent. Its tool execution broker must also enforce that an agent execution
can use only its assigned event or continuation context. Accepting a model-chosen
agent ID or sharing all users' reply capabilities in one sandbox is insufficient.
This authorization remains provider-owned; the gateway validates the bound reply
capability without maintaining a user-to-agent map.

Any generic provider notification must distinguish system events from verified
human events. Bot Father login callbacks, workflow progression and all signed-in
user information remain entirely in CMA.

## Acceptance evidence

- A greeting, CMA question or general research request runs through the ordinary
  agent with no forced setup flow.
- An unauthenticated resource request receives a CMA-owned login link. Browser
  authorization automatically resumes exactly the initiating agent.
- Two simultaneous users receive different agents/worktrees and cannot read or
  operate each other's credentials, resources, pending actions or conversation.
- The same sender returns to its existing agent after a gateway/API restart;
  duplicate first events do not create duplicate instances.
- Personal and organization creation use the correct owner, service inventory,
  connection prompts and agent-key scopes.
- Existing-agent attachment routes all senders to that instance; channel deletion
  leaves the borrowed agent and its environment intact.
- Profile-target dispatch lets the provider select and provision each user's
  durable instance. Gateway replicas neither choose that instance nor retain the
  user-to-agent mapping. Capacity exhaustion cannot spill a user into another
  user's instance.
- Callback retries, revocation, late login, busy turns and expired reply references
  produce explicit results without cross-user continuation or duplicate creation.

## Delivery status

PR [#953](https://github.com/ChronoAIProject/cma/pull/953) merged into `develop` at
`9ecf0eb9b42a50a842845dc5d10b84522832e496` and its CI-tested gateway image was
deployed to `chrono-bot` in production. CMAEG remains one replica. Both
`cma_codex` and `nyxbot` are registered; public gateway docs and OpenAPI return
200. The NyxID `nyxbot` agent-card endpoint returned 404 during verification, so
complete integration with that provider is not yet verified.

The agent-owned correction implements normal agent dispatch, actor-attested
setup actions, browser consent and a durable callback outbox, general delegated
operations, provider-owned source-user affinity, existing-agent attachment and
isolated per-user runtime launches. These changes are not included in the PR
#953 rollout. Their production activation is pending completion of the correction
branch's full CI and the rollout evidence below.

Local validation includes 195 CMA authentication tests, all six real MongoDB
authentication/outbox tests, all 988 core tests, the existing-agent index migration
regression, provider routing and transport-context preservation, seven CMA
callback/broker integration tests, gateway target and delayed reply HTTP tests,
2,124 TypeScript contract cases and 2,119 Rust SDK contract cases. All 20 isolation tests passed inside the retained Bot
Father production runtime without replacing it. Those checks establish their
individual boundaries; Telegram login/continuation and multi-user deployment
acceptance must still be recorded after activation.
