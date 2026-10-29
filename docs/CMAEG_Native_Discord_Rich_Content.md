# Native Discord sticker, embed and poll intake

Native Discord previously required plain-text content or an ordinary attachment.
A human message containing only a sticker, embed or poll was therefore retained
but refused as unsupported content. The independent
`CMAEG_NATIVE_DISCORD_RICH_CONTENT_ENABLED` writer gate permits newly received
messages to project supported facts into the ordinary agent message. It defaults
to `false` in code and the checked-in manifest.

## Provider contract

The implementation follows Discord's documented API/Gateway Message payload,
reviewed on 29 October 2026:

- [Message object and embed structures](https://docs.discord.com/developers/resources/message#message-object)
  describe optional `sticker_items`, `embeds` and `poll`. Message content intent
  controls visibility of embeds and polls; enabling CMA's gate cannot recover
  fields Discord did not send.
- [Sticker item](https://docs.discord.com/developers/resources/sticker#sticker-item-object)
  supplies `id`, `name` and `format_type`. Values 1–4 mean PNG, APNG, Lottie and GIF.
  Deprecated full `stickers` objects are not another intake path.
- [Poll object](https://docs.discord.com/developers/resources/poll#poll-object)
  supplies question, answers, expiry, multiselect and layout. Answer IDs are kept
  exactly; the implementation does not assume contiguous or sequential IDs.
  Default layout 1 is supported. Null expiry and emoji-only answers are accepted
  when the supplied answer otherwise has valid descriptive facts.

These are descriptions of received content. They do not add a sticker renderer,
image download, poll creation or vote operation. Discord explicitly does not allow
apps to vote on polls. Poll results can be absent or provisional; this projection
does not interpret them as zero votes and does not claim live results.

## Facts, attribution and bounds

Each sticker includes its ID, name and known format. Embed facts include its
known type, title, description, claimed author/provider name, footer and fields,
plus the presence of image/thumbnail/video/link content. URLs and proxy URLs are
not copied into the projection or converted to attachment locators. No network
request is made to rich-content URLs. Existing ordinary attachments keep their
exact Discord channel/message/attachment locators.

Poll facts include question, answer IDs, answer text and supplied emoji
identities, multiselect and expiry. The native human author remains the actor.
Descriptions explicitly identify the original sender/message and label embed
attribution as untrusted content, rather than changing sender authority.

Parsing is bounded to three sticker items, ten embeds, 25 fields per embed and ten
poll answers. Each embed has a 1 KiB text budget; poll question/answer fields have
individual bounds within a 6 KiB budget. The projected suffix is at most 16 KiB and
the whole Message remains at most 32 KiB. Original text is retained in full and
takes priority when space is limited. Truncated supported fields and omitted
metadata are identified in the projection. Unknown, malformed or empty metadata
is not enough to turn an otherwise empty message into agent work.

All bot, self, webhook, system, message-type, sender, chat and group admission
checks remain ahead of using the projected text. Embed authors and metadata
mention strings never satisfy a sender allowlist or Discord mention requirement.
Thread partitioning still requires separately verified exact channel evidence;
`MESSAGE_CREATE.thread` is not used as the containing thread.

## Durable replay and rollout

New eligible inbox bodies retain an optional version-1 `content_projection` with
typed facts and original message/channel/author IDs. The original authenticated
dispatch remains unchanged in the encrypted inbox. Existing bodies without this
field always use legacy normalization; a deployment cannot change an already
admitted mixed message's Activity digest or revive a previously refused rich-only
message into new work.

Same-native-message retries restore the retained body before considering current
writer settings or parsing the redelivered metadata. Admission acknowledgements,
routed identity and provider replay remain unchanged. The reader works when the
writer is off. V1 parsing/rendering is part of durable identity; a future semantic
change must introduce a separately versioned reader and writer.

Older inbox readers reject unknown body fields. Deploy compatible readers with
the gate off, drain all predecessor processes, then enable this gate explicitly.
Keep a compatible binary for recovery and rollback while any projected inbox
remains retained, including pending entries; disabling issuance alone is not a
safe rollback to an incompatible reader. The existing seven-day inbox retention
and 30-minute admission deadline are unchanged. Authenticated connection
capabilities expose optional `native_discord_rich_content_enabled` so operators
can confirm the configured gate without treating it as end-to-end verification.

Metadata-only updates, including embed refreshes and poll-result updates, do not
create new agent turns. The existing message-change path still requires a known
admitted original and a valid text/attachment edit revision. It retains the
original consumers, conversation and author. This feature grants no additional
edit, deletion, reaction or poll authority.
