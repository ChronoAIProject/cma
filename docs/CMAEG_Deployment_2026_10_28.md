# Event Gateway production rollout — October 28, 2026

PR #1171 merged to develop as `0d1772cfcbda4b55edba6f872495413ec9d6e72d`.
All 15 jobs in CI run `40699201203` passed. Its tested merge source
`615c6f23bbf3b9ea250bc77d72888aa19727b8b7` has the same tree as the develop
merge (`674ab5a55000aa49433715f0668df743488fe2fb`). Production uses those
tested GitHub CI artifacts; no local image build was used.

| Component | Immutable image digest | CI artifact |
| --- | --- | --- |
| Event Gateway | `sha256:03df804694cd5f43f637a44ebaa6002c988c4dbaaeb37beeee5e906943b79130` | `12821784343` |
| Web | `sha256:4ae30072a10c97e81e6c0a37a6799b2b2c2a884404e1eafff94c06e8cbea5803` | `12821347780` |

Gateway rollout completed around 23:00 SGT. Web rollout completed later that
evening, adding the source controls and capability labels from the same release.
Web keeps its prior image's assets plus every previously retained asset image
for existing tabs. Agent Manager and the runtime default were not updated by
these releases.

## Enabled capabilities and verification

This release enables native Lark controls, forms and conversation history;
Discord conversation history; Telegram and Discord supported media edits; and
WhatsApp incoming message changes. Source consent, configured credentials and
platform permissions still apply. Existing sources do not acquire new opt-ins.

Authenticated canonical capability discovery returned HTTP 200 and matched an
independent NyxID CLI request. Health, readiness and the same-domain OpenAPI
document returned HTTP 200. Media-edit gates do not have independent discovery
booleans; their enabled values were checked in the live configuration.

Telegram topic lifecycle, Discord thread lifecycle, WhatsApp Forms and WhatsApp
Flow writes remain disabled. Fresh production metadata checks verified their
optional stores, strict validators, non-TTL indexes, migration markers and
effective permissions. No further migration or grant write is needed. Their
activation is pending the remaining Custom regression correction below.

## Custom regression and known remaining defect

An isolated Custom campaign verified ordinary replies, a real 29-byte attachment
read, invalid-HMAC rejection (401), immediate Activity, and reconnect without an
additional provider turn. Activity appeared after 216 ms in the SSE observation;
this is a single production measurement, not a latency guarantee.

An additional exact duplicate-event replay exposed a legacy Custom path defect:
the Gateway regenerated event context under an unchanged idempotency key; Agent
Manager correctly rejected the changed request with 409, which the Gateway
projected as 503. The original response remained retained and no duplicate turn
started. This is an outstanding replay defect, so the aggregate acceptance is
**not passed**. The isolated channel and its exact diagnostic resources were
retired and its one-day key revoked; existing user channels were not edited.

Connected native platform message delivery was not exercised. These receipts
do not establish that every messaging scenario on all four platforms is ready.
The [capability inventory](CMAEG_Four_Platform_Completeness.md) records the
implemented boundaries and remaining gaps.

## Continuity and provenance

After Web rollout, 17 runtime Pod identities/restart counts/images/attachments
and 29 volume identities were unchanged from the post-test baseline. Forty-four
Agent lineages and 2,514 sampled durable events were retained. Seven referenced
public assets from the preceding Web entrypoint remained byte-identical and
available. The only preceding baseline difference was the explicitly owned
Custom diagnostic sandbox's cleanup, before rollout.

The Gateway publication receipt SHA-256 is
`023e22a19c42cf9736bbc1c8dc21f7727da75cfdebaad61b77a5ecf975bf6baa`.
It records retrospective registry/source verification and explicitly does not
claim a new archive download or unretained transfer log. The first live Gateway
patch mistakenly used the registry digest in the publication-receipt annotation;
the source manifest corrects that annotation. The live template annotation still
requires correction at the next Gateway rollout; the image digest itself was
verified independently.

The Web publication receipt SHA-256 is
`11cb89cf12610082733bc42e51d35c520c596615a7c6e3227f5cf898a3eee0db`.
The downloaded archive hash, image config, every layer and published registry
manifest/config were verified before deployment.
