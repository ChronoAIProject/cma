# Native Discord forwarded-message intake

`CMAEG_NATIVE_DISCORD_FORWARDS_ENABLED` permits new native Discord human messages
to retain a bounded description of a received forwarded snapshot. The gate is
independent of rich-content intake and defaults to `false` in code and the manifest.
It requires the native runtime and routed recovery. This feature consumes facts
Discord already supplied; it adds no origin-channel reads or outbound forwarding.

## Provider contract and attribution

The [Discord Message reference and snapshot contract](https://docs.discord.com/developers/resources/message#message-reference-object),
reviewed on 29 October 2026, identifies a forward by `message_reference.type=1`
and currently supplies exactly one `message_snapshots` entry. Required reference
message/channel IDs are validated but never used as reply, read or effect targets.
The origin may be another channel or guild. The actual outer human forwarder,
current channel and separately verified current thread remain the CMA authority.

Discord explicitly excludes the original author from its snapshot. CMA states
that the original author is not provided; it does not infer one from mentions,
embed attribution, origin IDs or text. A claimed nested author or message identity
is unsupported. The accepted nested message types are DEFAULT, REPLY,
CHAT_INPUT_COMMAND and CONTEXT_MENU_COMMAND. Nested forwards, polls, calls and
activities are outside this profile. This does not change legacy normalization
of inbox bodies that lack the new projection.

The description contains supplied text and counts of attachments, embeds,
stickers and components. It does not expose nested attachment IDs, filenames,
URLs or component actions. Text may itself contain user-supplied links; these
remain quoted untrusted text and cause no fetch. Snapshot mentions never count
as addressing the CMA bot. The first slice has no nested media download, embed
rendering, component execution, outgoing forward or original-author lookup.

## Bounds and durable identity

Text is capped at 4 KiB on UTF-8 boundaries; unsupported control/bidirectional
characters are removed with a truncation marker. Descriptive counts are bounded
to ten attachments, ten embeds, three stickers and five component containers;
these are CMA description limits, not claims about provider limits. Over-limit
counts are marked truncated. Empty or malformed snapshots alone do not create
agent work. Original outer text and existing rich-content v1 descriptions take
precedence; the new suffix is at most 12 KiB and the entire message at most 32 KiB.

Eligible new inbox bodies retain a separate optional `forward_projection`
version 1. It binds the current message/channel/forwarder plus a canonical digest
of the full reference/snapshot pair, including fields omitted from display. The
original authenticated dispatch is unchanged. Existing `content_projection`
version-1 capture and rendering are unchanged, and the two optional projections
can coexist. Missing forward projection always means legacy behavior, even when
the writer is now enabled. A previously retained forward-only refusal cannot
become new work merely because the flag changes.

Same-native-message retries restore the winning inbox. For projected forwards,
a changed reference, supplied snapshot, current author or current channel fails
verification; an altered retry cannot replace the original snapshot or grant a
second admission. The fixed projected bytes remain readable after restart or
with issuance disabled. The ordinary routing manifest retains its exact original
default/named consumers, conversation IDs and seals. Current source revocation
still applies. Snapshot-only MESSAGE_UPDATE events do not start turns; existing
outer text/attachment revisions and message deletion use the original context.

## Rollout and compatibility

Deploy compatible readers with issuance off and drain predecessor processes before
enabling the gate. Older inbox readers reject unknown fields. Keep a compatible
rollback/recovery reader until all projected inbox entries expire; disabling the
writer does not make old readers compatible. The existing seven-day inbox window,
message-snowflake age enforcement and 30-minute admission deadline are unchanged.
No new schema is added to public Activity or agent tools.

The authenticated capability response and both SDKs expose optional nullable
`native_discord_forwards_enabled`. Absence does not imply support; false/true are
configuration visibility, not proof of native delivery. Local regression fixtures
cover parsing, attribution, no nested download authority, retention across gate
changes, altered retries, original targets and thread proof. Connected Discord
acceptance remains unverified for this increment and is a testing-coverage label.
