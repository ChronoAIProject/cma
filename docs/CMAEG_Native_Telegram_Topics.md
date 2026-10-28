# Native Telegram forum topic close/reopen

This source increment adds exact admitted-topic `close` and `reopen` through the
existing `manageThread` contract. It is disabled by default and has not been
activated or accepted against connected Telegram accounts.

The operator gate is `CMAEG_NATIVE_TELEGRAM_TOPIC_LIFECYCLE_ENABLED` (default
`false`). The channel owner separately enables `topic_lifecycle: true` on a native
Telegram source. This consent is independent of `message_events`; enabling topic
management does not retain incoming edits or reactions. The portal exposes
**Manage the original forum topic** and saves durable default-only routing
transparently for a previously unrouted source. Existing routing is preserved.
Durable admission/recovery, the multi-consumer authoring gate and a provider with
durable response replay support must be available.

## Scope and discovery

Only the exact non-General forum topic containing the original admitted human
message is supported. Authority comes from the retained native webhook bytes,
committed routing plan and original selected consumer, together with the current
source, owner, provider, credential and bot custody. Both default and named
consumers are supported. A source opted in after an event was admitted cannot
retroactively grant topic authority to that event.

Eligible event context may include:

```json
{
  "topic_lifecycle": {
    "manage_original_topic": true,
    "actions": ["close", "reopen"],
    "expires_at": 1800000600
  }
}
```

This optional member is inside `capabilities`. The existing Discord
`thread_lifecycle` member and sealed v1 `thread_ref` format are unchanged.
Discovery restores local custody without contacting Telegram, is bounded to
100 ms, and may omit the hint when storage or concurrency is unavailable. It
reports scope, not a current platform permission grant.

There is no Telegram list/create, caller-supplied chat/topic ID, `thread_ref`,
rename, private-chat topic or General-topic operation. Channel posts, edits,
callbacks, business/guest messages, anonymous `sender_chat` messages, bot actors
and relay-normalized input do not provide authority. General topic ID `1` is
excluded because Telegram provides separate General-topic methods.

## Existing REST, MCP and SDK operation

`POST /v1/thread-operations` requires one stable visible-ASCII `Idempotency-Key`
header (1–128 bytes), with this body:

```json
{
  "target": { "type": "event", "event_ref": "<original-event-ref>" },
  "action": "close"
}
```

Use `"action": "reopen"` to reopen that same topic. MCP `manageThread` uses
`{"request": <body>, "idempotency_key": "<stable-key>"}`. The existing TypeScript
`manageThread` and Rust `manage_thread` methods use the same request. Telegram-only
MCP installations advertise only `manageThread`, not Discord list/create tools.
Discord refuses the additive `close` action before any platform reads or writes.
Telegram refuses `archive`, `join` and `leave` as unsupported.

The gateway verifies its bot with `getMe`, confirms the exact forum supergroup
with `getChat`, and checks its own `getChatMember` administrator
`can_manage_topics` permission (or chat owner status). This bounded implementation
does not rely on Telegram's topic-creator exception because the retained human
message does not prove bot topic creation. It rechecks local custody immediately
before exactly one `closeForumTopic` or `reopenForumTopic` dispatch. A successful
boolean `true` produces `{"outcome":{"status":"confirmed"}}`; no new thread
reference is issued and the event lifetime is never extended.

Definite preflight refusal produces `rejected`, and platform rate limits produce
`rate_limited` with `retry_after`. An unconfirmed result after dispatch produces
`unknown`. The full request has an eight-second budget, with reserved time for
receipt persistence. No detached platform writer continues after that budget.

## Durable receipts and uncertain writes

Claims and fences use separate encrypted Telegram namespaces. The persistent
fence destination includes platform, bot, chat and topic; topic IDs are only
unique within a chat. Concurrent events, consumers and new request keys cannot
bypass an uncertain operation. Same-key replay returns the retained receipt;
reusing the key for a different action conflicts. Unknown dispatch, cancellation
or unpersisted completion keeps the fence held across restart and event expiry.
Never retry an unknown result using a fresh key. There is no automatic fence
expiry or reset endpoint.

The existing optional `eg_thread_fence_records` store supplies non-TTL fences;
claims retain the event's fixed expiry in the existing interaction store. This
increment adds no collection, migration or runtime grant. Missing optional-store
readiness disables source authoring and execution and suppresses discovery.
See [operations](CMAEG_Operations.md#native-telegram-topic-lifecycle-rollout).

## Existing message reference compatibility

Topic consent is omitted when false. Ordinary message-event locators, retained
received reactions and media-edit source custody ignore only this independent
flag, preserving their prior serialized source bytes. The topic operation itself
requires exact original and current opted-in source custody.

Automatic enrollment leaves all issuance hashes unchanged. Readers may validate
an older absent-routing sent-effect or media-edit authority only when its original
consumer is the default (`None`) and the complete current typed routing policy
exactly equals the automatic default-only policy. Additional consumers, targets,
rules, names, reply/publish/non-human overrides or addressing changes do not qualify.
Existing owner, source, admission, provider, credential and key checks remain in
force. Routing-bearing references retain their exact original hash checks. Topic
opt-out leaves the enrolled routing intact and ordinary references usable.

## Provider references

- [Telegram closeForumTopic](https://core.telegram.org/bots/api#closeforumtopic)
- [Telegram reopenForumTopic](https://core.telegram.org/bots/api#reopenforumtopic)
- [Telegram ChatMemberAdministrator](https://core.telegram.org/bots/api#chatmemberadministrator)

Public documentation was checked on 2026-10-28. Local tests use in-process
Telegram/CMA boundaries; they are not connected-account native acceptance.
