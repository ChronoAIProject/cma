# Native Lark / Feishu thread starts

When `getEventContext.capabilities.native_thread_start` is true, an agent can explicitly send `replyToEvent` with `thread_reply: true` and one plain-text `text` value to start a thread on the exact original group message. Text is limited to 1–4,000 characters and 28 KiB. Blocks, native attachments, templates, explicit reply targets and media updates are unsupported for this operation. No caller-selected message, chat or thread ID is accepted.

`CMAEG_NATIVE_LARK_THREAD_START_ENABLED` defaults to false and the manifest keeps it disabled. It also requires native intake and durable routed admission. An explicit outbound intent supplies consent to reply to the already admitted message; no conversation-history consent or generic Discord thread contract is reused. Existing ordinary sends, existing-thread replies, media cards, callbacks and card recovery keep their authority and behavior.

Both regions document thread creation as a reply: `POST /open-apis/im/v1/messages/:message_id/reply` with `reply_in_thread: true`. CMA verifies the original human/group/root context and current native app/bot, then reads the exact original message. The metadata must confirm the original author, tenant, chat, creation time and a root with no foreign parent. If that same root has acquired a thread since admission, its verified current thread may be continued. A different message's thread cannot be adopted.

After those reads, current source revision, consumer and provider authority is checked immediately before the write. One successful response must identify a new app-authored text message, the exact original chat/root/parent and a valid thread ID; an already-known thread must match exactly. The durable native receipt retains that destination, original consumer/provider, partition and conversation. If message effects are enabled, the confirmed text reference grants effects only on the new message in that confirmed thread. The original event and its conversation are not rewritten.

Rejected/unsupported thread operations never fall back to top-level chat delivery. Malformed success, network uncertainty or uncertain receipt persistence produces no usable message reference. The existing durable tool ledger retains confirmed and unknown outcomes, so replay/recovery cannot send the same operation again. The optional upstream UUID is not used as a substitute for CMA's durable ledger.

This creates a reply and thread together, not an empty named container. It does not grant Discord-style name/auto-archive, thread enumeration, rename, membership or forwarding capabilities. Group settings, visibility, bot permissions and provider rate limits still apply; documented error codes 230071 and 230072 reject unsupported groups or aggregated messages.

## Official contracts reviewed 2026-10-29

| Region | Thread guide SHA-256 | Reply API SHA-256 |
| --- | --- | --- |
| Lark | `a7b0ba2ca62dbd086e5fafa7cea0331e148901fc366dbb1b7b59a3f601aff96c` | `bf8db79f334a03951bd51b5bc69fb7a09fe51f22c95a083fcf1195039848778f` |
| Feishu | `6ff6c9b99912d114edc71518e72cbe8e441ca287098eddc7803e9886ee35ca9c` | `7c7e7210d6049616282ed891bbb0932cc090e98e0e43a78d34c8ed196d005944` |

Guides: [Lark](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/thread-introduction.md), [Feishu](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/thread-introduction.md). Reply methods: [Lark](https://open.larksuite.com/document/server-docs/im-v1/message/reply.md), [Feishu](https://open.feishu.cn/document/server-docs/im-v1/message/reply.md). The guides explicitly describe “reply as a thread (create thread)”.

Incoming edit notifications remain unverified. Official receive envelopes contain `update_time`, but current public catalogs/SDKs do not establish an edit delivery and revision contract. CMA does not infer edits from changed retries. Local transport tests cover this feature's contract; real platform acceptance still needs an authorized connected Lark/Feishu test.
