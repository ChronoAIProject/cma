# CMA Bot Father: agent ownership and user sessions

This document records the maintainer's 2026-09-28 requirements and the remaining
implementation boundaries. It is a target design, not a claim that all flows are
deployed. Public gateway contracts remain in the gateway's own
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
interpret `/setup`, decide whether a message requests a Bot, run a setup state
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

The protocol must identify the target explicitly. A fixed partition on a profile
currently creates one new profile instance; it does not attach to an existing
agent. Do not advertise that as existing-agent support. Providers must reject
unsupported target types rather than silently provision a different resource.

The gateway preserves authenticated platform/sender, source chat/thread, event ID
and reply destination as event metadata. It does not hash the sender into an
agent-selection key, maintain a user-to-agent map, or decide whether an agent
needs creating. A verified Telegram sender is a source identity; it is not proof
of a NyxID account. CMA owns the authenticated account association after login.

A subconnection is a provider-owned session beneath one channel binding. For
Bot Father, CMA durably associates the channel and verified source user with an
Agent, Worktree, conversation and delegated identity. CMA defines the affinity
policy, including whether the same user across multiple chats shares an agent.
The gateway treats provider session identifiers as opaque correlation metadata.
Each event retains its own reply destination regardless of which agent is selected.

Repeated events, retries, gateway restarts and simultaneous first events must
resolve to the same provider-selected instance. This guarantee must be enforced
by CMA's durable mapping and admission logic, not by a gateway sender partition.
The existing gateway `partition` fields are compatibility behavior of the current
protocol; adding a new gateway `sender` rule does not implement this requirement.
The profile-target provider contract must let the provider choose the session
without treating a gateway-derived conversation ID as the user-affinity authority.

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

The current link flow records a browser candidate and requires a one-time proof
from the original Telegram sender. Its return link is an interim flow, not the
automatic callback required here. NyxID's existing `telegram_widget` provider API
can expose a server-verified Telegram user ID, but production inspection on
2026-09-28 found its widget bot username unconfigured. The implementation must
not assume that this optional proof path is available, or change NyxID to enable
it without authorization.

## Delegated operations and isolation

General CMA operations require a reusable delegated operation interface; the
setup-specific action API alone does not fulfill this requirement. Each operation
must resolve its subject and grant from the authenticated agent/subconnection
context, check the grant and current resource authority, and use existing CMA
resource services and command admission. A model-supplied subject, owner or grant
ID is not sufficient authority.

The credential broker keeps user secrets outside the shared sandbox. Per-user
history, draft configuration and tool results must never be added to another
agent's prompt. Shared skills and CMA documentation may be reused. User-specific
memory and grants may not be shared.

Separate worktrees and `CODEX_HOME` directories currently separate ordinary model
contexts, but all sandbox processes share a Unix identity and filesystem. They
are not a security boundary against sibling agents with shell access. Production
multi-user acceptance therefore also needs enforced runtime access separation
and agent-scoped tool authority within the shared Sandbox. Merely placing tokens
in different files does not meet this requirement.

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

The proposed generic extension is an opaque `reply_target_ref` minted from
verified ingress, separate from the provider-selected session identifier. It
authorizes delivery to the original source conversation/topic under the current
channel and provider binding, with explicit lifetime and revocation semantics.
The platform adapter may retain an original message reference when its relay
requires one. Its scope contains delivery authority, never a user's NyxID grant
or an agent-selection policy. It must not become an unrestricted arbitrary-chat
send credential.

A proposed `POST /v1/reply-targets/{reply_target_ref}/messages` operation would use
that target and a provider-supplied idempotency key. This is a proposed extension,
not an existing endpoint. Every agent uses the same API; the authenticated target
determines the destination. Expired/deleted/replaced channel authority fails
closed. Provider/gateway delivery receipts must distinguish confirmed, rejected
and unknown delivery; an unknown platform send cannot be labelled exactly-once.

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

## Delivery status on 2026-09-28

PR [#953](https://github.com/ChronoAIProject/cma/pull/953) merged into `develop` at
`9ecf0eb9b42a50a842845dc5d10b84522832e496` and its CI-tested gateway image was
deployed to `chrono-bot` in production. CMAEG remains one replica. Both
`cma_codex` and `nyxbot` are registered; public gateway docs and OpenAPI return
200. The NyxID `nyxbot` agent-card endpoint returned 404 during verification, so
complete integration with that provider is not yet verified.

The agent-owned correction branch removes gateway Bot Father interception and
adds typed, actor-attested setup actions. Those are intermediate changes, not
evidence for automatic callback continuation, general delegated operations,
existing-agent attachment or secure multi-user runtime isolation. They were not
included in the PR #953 production rollout.
