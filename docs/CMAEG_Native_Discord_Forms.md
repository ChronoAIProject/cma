# Native Discord forms

CMA's native Discord transport can present one neutral `Form` as a button that
opens a modal, then submit the complete form to the card's original consumer.
New issuance requires `CMAEG_DISCORD_INTERACTIONS_ENABLED=true` and
`CMAEG_DISCORD_FORMS_ENABLED=true`; named consumers also require
`CMAEG_CONSUMER_INTERACTIONS_ENABLED=true`. The form gate defaults to false,
including in the checked-in CMA configuration. NyxID relay Discord controls
remain outside this path. There is no new public API field or callback endpoint.

## Representation and compatibility

The native subset is one form containing 1–5 fields, with unique field IDs,
labels of at most 45 Unicode characters, and no independent Buttons/Choice
blocks, reply-reference override or native attachments. The launch message must
fit one ordinary Discord text part. Existing verified threads are supported;
this grants no thread creation, membership, archive or history operation.

| Neutral field | Modal representation | Canonical submitted value |
| --- | --- | --- |
| Text | Paragraph TextInput, at most 1,000 characters and 4,000 UTF-8 bytes | JSON string |
| Choice | StringSelect inside Label; 1–20 unique options and at most one selected default | Authored option ID, never the callback's opaque index |
| Number | Short TextInput labeled by the field with `Number (e.g. 12.5)` placeholder; at most 64 bytes | Finite JSON number; no surrounding whitespace, comma separators, coercion, NaN or Infinity |
| Date | Short TextInput labeled by the field with `Date (YYYY-MM-DD)` placeholder | Exact valid ISO calendar date, `YYYY-MM-DD` |

Discord has no native number or date widget. A form containing either is honestly
reported as degraded with `discord_modal_number_date_validated_text`; Text/Choice
form rendering is exact. Other blocks retain their own rendering report. Choice
option labels are at most 100 characters; only default option styling is accepted.
An optional empty field is omitted from the returned map. Missing required,
duplicate, unknown, malformed or extra fields reject the whole submission.

The modal uses current Label components (type 18), wrapping TextInput (type 4)
or StringSelect (type 3). Its launch uses a legacy ActionRow/button, with no
`IS_COMPONENTS_V2` flag. Business field IDs and option IDs are stored on the card;
Discord custom IDs contain only card/revision/attempt locators and `f0`–`f4` keys.

With the form gate off, existing Form-only delivery remains ordinary text. With
the gate on, valid forms outside the native subset (including more than five
fields, long labels, mixed controls or multi-part text) retain exactly their
previous delivery and degradation. Mixed existing controls retain their existing
control behavior plus plaintext form rendering. Eligibility is decided before
native state or platform writes. A failed authority check, native record write,
or uncertain platform response never falls back to another send.

## Two-stage custody and recovery

1. A verified type-3 button click checks the actor, source, current provider,
   consumer, destination, card revision and existing thread. It saves a
   non-consuming launch, then durably claims a single type-9 response before
   opening the modal. Launching creates no provider turn or inbox Activity.
2. A verified type-5 modal submission must match that launch and its guild. A
   missing component-origin message uses the confirmed retained destination;
   a supplied message must match it. Fields are decoded as a complete canonical
   set before the card's single consume CAS.
3. After consumption, CMA persists its normal sanitized native inbox and
   original single-consumer Activity before one type-6 ACK attempt. The existing
   dispatcher executes that same conversation and provider exactly once.

Initial responses use the existing conservative 2.5-second snowflake budget,
inside Discord's 3-second deadline. CMA does not defer then open a modal, respond
to a submission with another modal, or retry an uncertain initial callback.
Interaction tokens never enter the retained inbox, card or provider request.

An invalid field set does not consume the card or invoke a provider. The fixed
ephemeral response is: `Please check the form values and open the form again to
retry.` The rejection identity is retained so altered replay bytes cannot become
a valid submission. The user explicitly clicks **Open form** again; that fresh
launch supersedes the older attempt and may submit successfully. The modal
attempt expires after ten minutes or the card's earlier expiry. A cancelled
modal likewise leaves the card unconsumed.

A lost type-9 response or dispatch-claim result never resends the same response.
A fresh launch remains possible. A genuine subsequent submission may settle a
claimed type-9 whose response was uncertain. Exact consumed submissions recover
lost inbox writes after the response deadline and attempt expiry without
re-consuming, renewing authority or issuing a second ACK. Provider/source
revocation still denies recovery. Edit and deletion share the original card CAS:
if they win first, an unconsumed old modal cannot execute; if consumption won,
that exact result remains recoverable. Edited successors reset the launch and
consumed result and bind new controls to the successor revision.

The provider receives existing neutral `InteractionResult` data:

```json
{
  "block_id": "request",
  "action_id": "request",
  "values": {
    "fields": {"name": "Ada", "amount": 12.5, "date": "2028-02-29"}
  }
}
```

Existing message reference, record, revision and platform-message metadata remain
in `values`. Buttons/Choice results keep their existing `selected` shape.

## Rollout and evidence

Keep issuance off until all serving and rollback readers understand optional
Discord form authority, launch records and consumed whole-form results. These
are internal encrypted records, omitted on historical cards; the strict Discord
identity reader fails closed on older code. Once any native form is issued,
rollback must retain compatible readers. Turning issuance off preserves retained
modal callbacks, recovery and form edits; adding native forms to an older card
still requires the form gate. Existing edit/effect gates continue to apply.

Protocol shapes were checked against official
[`discord/discord-api-docs@6128d99116af8357db1e4966ebd7f099817a0e04`](https://github.com/discord/discord-api-docs/tree/6128d99116af8357db1e4966ebd7f099817a0e04),
retrieved 27 October 2026:

- `developers/interactions/receiving-and-responding.mdx`, SHA-256
  `f9c9efe2d882830ae14649f22ee1e33a156ac789eeba5174ca9fb72d34c99a13`.
- `developers/components/reference.mdx`, SHA-256
  `ae1dfe6095de9c7ab00e9e04cf46d4e42dad5e02280dcdbea375f63a3fa91a66`.
- `developers/components/using-modal-components.mdx`, SHA-256
  `49c03865b9143f5ae7d25cfd5dd6be05ac396a0998787ee200af84b4b586c7cb`.

Local encrypted-record, HTTP callback and real-provider fixtures cover composed
launch/submission, canonical fields, failed-value retry, lost response/record
recovery, current identity rejection, default-off degradation, edit/delete races
and existing-thread routing. This is local contract verification. Real Discord
client rendering and submission acceptance remain unverified; no live Discord
message or production form rollout is part of this change.
