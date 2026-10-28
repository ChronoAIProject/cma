# Native Lark and Feishu forms

This document describes the NyxID relay form path. The separate native app uses [native Lark controls and forms](CMAEG_Native_Lark_Controls.md) with independent flags, locators, receipts and callback admission.
Native Lark and Feishu cards can collect one neutral `Form` block and submit its
fields together. New forms require both `CMAEG_LARK_CARDS_ENABLED=true` and
`CMAEG_LARK_FORMS_ENABLED=true`. The form flag defaults to false. Named consumers
also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED=true`.

Deploy compatible serving and rollback readers before enabling the form flag.
New cards retain optional form authority, and consumed clicks retain an optional
`submitted` result. Historical card and button-click fields remain absent, so
their serialization remains unchanged. Form callbacks use the distinct `clf1:`
locator; older readers cannot interpret it as a button. Turning the flag off
stops new native form issuance. Existing plain-text form rendering remains the
fallback, and retained native forms remain readable, consumable and editable
through their saved authority. Editing a historical plain form preserves its
plain rendering even if the native form gate is later enabled.

## Supported form contract

A native form has one to twelve fields and one generated **Submit** button. Field
IDs must be unique. Only one native Form block is supported per card, and it cannot
be combined with independent Buttons or Choice blocks. Text, summary cards and
links retain their existing Lark rendering. Native attachments, WhatsApp
templates, threaded cards and published-agent callback audiences are unsupported
for native form issuance. Enabling the flag selects a native form only when both
structural preflight and the existing native-control context checks succeed.
Unsupported native form shapes and valid but ineligible contexts retain their
previous delivery behavior and rendering report. Form-only replies fall back to
ordinary text; a Form mixed with Buttons or Choice retains the existing native
controls and `plain_text_form` degradation when those controls are eligible.
Threaded, published and nonhuman contexts keep their prior text reply destination.
Fallback forms receive no native form callback authority. Form-only replies with
unsupported native shapes do not fetch provider context. Contexts known to be
ineligible without a provider read also avoid that read.
With the form flag off, Form-only replies likewise need no provider context read
or interaction record. Existing media and unavailable-thread refusals still apply.

Invalid authorization, mismatched or unavailable provider context, and durable
storage failures remain fail-closed. They never select a fallback delivery path.
After a durable write or send begins, an uncertain outcome cannot cause a text
fallback or a second send. This fallback policy applies to new replies; retained
native forms keep their exact saved schema and authority for edits and replay.
Telegram and plain relay form-text updates keep their existing behavior, including
named-consumer edits while the consumer-interaction gate is off.

| Neutral field kind | Native component | Submitted result |
|---|---|---|
| `text` | Multiline input, at most 1,000 characters and 4,000 UTF-8 bytes | JSON string |
| `number` | Text input, at most 64 bytes; server validates JSON number syntax | Finite JSON number |
| `date` | Date picker | Valid calendar date as `YYYY-MM-DD` |
| `choice` | Single-choice menu with 1–20 options | Stored business option ID as a JSON string |

Choice IDs must be unique and nonempty, with at most one selected default.
Non-default choice styles are ineligible for native issuance because the menu
cannot represent them; new replies retain their prior text rendering.
Choice IDs are bounded to 512 bytes and labels to 200 bytes. The existing card
limits still apply: 4,000 display characters, 28 KiB card JSON, and bounded
components. A submitted field object and its canonical result are each limited
to 16 KiB.

The platform sees opaque names such as `f0` and opaque choice values such as `1`.
Provider field IDs and option IDs are resolved only from the encrypted card.
Unknown or noncanonical field names, unknown choice indices, arrays, nested
objects, invalid dates and invalid numbers are refused. Required fields must
have a nonempty value. Omitted optional fields, `null`, and empty strings become
absent result fields. Text is preserved exactly. Numeric inputs must use JSON
number syntax without surrounding whitespace. Date-picker values may include
the documented numeric timezone suffix; the result retains the calendar date.

The resulting Activity uses the existing `InteractionResult` contract:

```json
{
  "type": "interaction",
  "block_id": "request",
  "action_id": "request",
  "values": {
    "fields": {
      "amount": 12.5,
      "day": "2024-02-29",
      "reason": "Ready",
      "scope": "private-write"
    }
  }
}
```

Both IDs are the authored Form ID. `values` additionally includes the existing
message reference, interaction record ID, card revision and platform message ID.
Form results use `fields`; existing button and choice results continue to use
`selected`. The neutral Form and open `InteractionResult.values` types already
exist in the public contracts, so no SDK or OpenAPI shape change is required.

## Callback authority and recovery

The existing authenticated NyxID relay carries both normalized callback text and
the raw `card.action.trigger` event. CMA requires them to agree exactly on the
action tag, opaque locator, submitted fields and original message ID. The native
JSON 2.0 callback uses tag `button` with a non-null `form_value`; the pinned NyxID
fixture's `form_submit` tag is also accepted for a native form locator. Ordinary
button locators continue to reject form values.

Before consuming a form, CMA checks the original human actor, exact conversation,
platform, relay route, delivered message, card revision, confirmed delivery and
expiry. Current source admission still applies. The form also retains its
original provider binding, default or named consumer, conversation ID, partition
and source-definition digest. Changing the source definition or rebinding the
provider revokes the form. A consumer rename or routing/partition change cannot
move its callback to another consumer or conversation. Removing or disabling the
selected consumer refuses it.

CMA validates and saves the entire canonical field map with the consumed click
before dispatch. Retries must match the original callback ID, logical event,
action index and canonical submitted result. A changed field value or a competing
callback cannot replace the winning submission. The retained timestamp and
result produce stable Activity and provider idempotency across restart. Repeated
storage CAS contention remains retryable with HTTP 503; it does not turn a valid
form into a terminal refusal.

Cards that previously rendered forms as plain text have no native form authority
and cannot be upgraded by a forged `clf1:` locator. A consumed native form can be
replaced with another supported form through its saved authority even when new
issuance is disabled, or with ordinary card content. A pre-form card cannot be
edited into a native form: issue a new reply from an authorized event instead. Replacement still fences the old card
before the external edit, and an uncertain receipt does not reactivate it.

## Contract evidence and acceptance

The audited NyxID source is revision
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`. Its Lark-family adapter passes
`metadata.card` through to native interactive messages; `parse_card_action_event`
retains `tag`, `value`, `form_value`, operator, chat and message identity. Its
`parse_card_action_form_submit` fixture covers Feishu submission transport. This
implementation changes CMA only.

The JSON 2.0 schema follows the official Feishu documentation for the
[form container](https://open.feishu.cn/document/feishu-cards/card-json-v2-components/containers/form-container),
[input](https://open.feishu.cn/document/feishu-cards/card-json-v2-components/interactive-components/input),
[single-choice menu](https://open.feishu.cn/document/feishu-cards/card-json-v2-components/interactive-components/single-select-dropdown-menu),
and [date picker](https://open.feishu.cn/document/feishu-cards/card-json-v2-components/interactive-components/date-picker).

Local signed-callback, durable-store and recorded-delivery fixtures establish the
CMA contract. Actual rendering and submission acceptance on Lark and Feishu are
separate regional platform checks. The default-off manifest does not claim those
checks or activate native form issuance.
