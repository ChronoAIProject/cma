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

CMAEG authenticates and normalizes source events, selects a provider conversation,
invokes the provider, and delivers replies and generic interactions. It does not
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

A channel target and its partition policy are separate concepts:

| Target | Conversation behavior | Resource lifecycle |
| --- | --- | --- |
| Existing agent instance | Every admitted event reaches the specified agent, regardless of sender. Require a fixed partition. | Attach to the authorized existing instance. Deleting the channel detaches it; it must not delete the borrowed agent, workspace or sandbox. |
| Event Worker Profile | A partition selects a durable instance of the profile; the first event for a new partition reserves and creates that instance. | The provider owns provisioning and cleanup of the instances it created. |

The protocol must identify the target explicitly. A fixed partition on a profile
currently creates one new profile instance; it does not attach to an existing
agent. Do not advertise that as existing-agent support. Providers must reject
unsupported target types rather than silently provision a different resource.

For sender routing, the stable key includes the source platform and authenticated
sender ID, with the channel identity included in the provider conversation ID.
The chat ID is excluded. Existing `conversation_and_sender` routing remains
available when one sender should have independent conversations in different
chats. Changing partition semantics requires a new partition-rule version.

A subconnection is the durable association of channel, partition-rule version
and partition key with one provider conversation. Repeated events, retries,
gateway restarts and simultaneous first events must resolve to the same instance.
The gateway transports opaque provider identifiers; CMA owns the mapping to its
Sandbox, Workspace, Agent and Worktree.

Bot Father uses the profile target with a separate sender subconnection for each
user. CMA retains one always-live Sandbox and Workspace, with a distinct Agent,
Worktree, model conversation and per-user operation context for each subconnection.
Different agents may run concurrently; turns for one agent remain ordered.

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

A callback can arrive after its initiating turn completed. CMA therefore needs a
generic way to resume its provider conversation and send a result to the retained
source destination. Current short-lived event references are not indefinite
conversation credentials. Do not disable expiry to make delayed callbacks work.

Any necessary gateway extension must describe a generic provider notification or
conversation reply target, authenticated to the owning channel and current
provider binding. It must distinguish system events from authenticated human
events, preserve sender and destination separation, and expose no user login
state. Bot Father callback handling belongs entirely in CMA.

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
- Profile-target creation provisions one durable instance per selected partition;
  capacity exhaustion cannot spill a user into another user's instance.
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
