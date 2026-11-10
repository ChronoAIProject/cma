# Reservations before CMA model requests

This extends the [owner/turn budget foundation](CMA_Model_Budget_Controls.md)
for the model requests CMA sends directly: automatic titles and contextual
suggestions. Both are tied to a completed Agent turn. There is no separate
control-plane summary client in the current source; new clients must use the
same reservation boundary. Runtime/provider requests require the gateway
integration tracked by #1496/#1566 and are not covered by these clients.

## Private configuration and request bounds

Activation remains off by default. The owner supplies limits and valuation
privately under #1483; the lead must relay activation direction. An enabled
owner also requires an entry in `model_budget.request_models` for each exact
model used by the broker. No model, rate, allowance or framing bound is enabled
implicitly. The existing admin settings capability, revision CAS and audit
contract applies to this section.

Each model entry names `model` and `input_overhead_tokens`. Before approving a
model, the operator must establish that its text tokenizer uses no more than
one token per UTF-8 byte and that the configured overhead covers protocol
framing and any provider-added input. The estimate counts the entire serialized
request, including instructions and schemas, plus that overhead. Models without
this verified bound must remain absent. Non-text inputs, tools and unbounded
continuation are outside these auxiliary clients' contract.

The owner policy's input and output valuation must conservatively cover the
highest applicable rates for all admitted models. Quotation uses checked exact
integer arithmetic: input estimate times input valuation plus the enforced
maximum output tokens times output valuation. The clients bound request size,
pin the model and output limit, disable redirects and automatic HTTP retries,
and never accept provider routing supplied by a user.

## Durable hold and settlement

`RequestBudget` is the neutral reserve/settle interface. Its Mongo adapter stores
request holds and a turn aggregate in the existing `model_budget_reservations`
collection. Prompt content and credentials are excluded; only a request digest
and accounting scope are retained. No new collection or TTL is introduced.

The store derives owner scope from the persisted Agent and checks the originating
turn's reservation. A settings write fences policy changes. A shared owner-row
write and turn aggregate serialize concurrent holds across replicas. Admission
checks both the owner/day budget and the remaining originating-turn budget,
including earlier auxiliary requests and unresolved holds. Existing parent-turn
accounting remains conservative: it retains its full debit when authoritative
final main-turn usage is unavailable. Auxiliary holds are additional liability;
this can refuse work earlier than a fully reconciled meter would.

Only a fresh, confirmed reservation transaction returns permission for one HTTP
send. A repeated request key never grants another send. Unknown commit outcomes
refuse sending; a possibly committed hold remains accounted. A title retry is
a new actual attempt with its own hold. Suggestions use their durable work key.
Every attempt therefore needs available capacity even after a timeout.

Budget-store unavailability, an unapproved model, missing turn binding, invalid
policy, arithmetic overflow and exhausted capacity refuse the request before
network dispatch. Title orchestration retains the typed budget reason and its
retryable unavailable result. Suggestions preserve their existing typed
unavailable failure shape for compatibility.

Complete successful provider responses with both usage counters settle the
exact hold transactionally. Duplicate identical settlement is harmless;
conflicting usage is refused. Missing, malformed, incomplete or mismatched-model
usage leaves the full hold outstanding. Timeouts, transport errors, restarts
and elapsed time do not release it. No cleanup timer treats uncertainty as free.

If recorded usage exceeds the certified bound, accounting records the actual
usage and blocks further auxiliary calls for that turn; the caller receives an
unavailable result. That event invalidates the configured bound and requires
operator investigation before any claim of proven enforcement. Unresolved
holds require authoritative reconciliation; deleting rows is not a reset.

## Verification and release

Local tests cover HTTP non-dispatch on budget refusal, reservation before send,
recorded settlement, missing-usage retention, concurrent replica admission,
replay/restart, unknown models, missing accounting authority, conflicting
settlement and bound violations. Tests use synthetic accounting units.

Production verification remains with the release operator using an isolated
test owner and privately supplied conservative settings. Record source/artifact
identity, settings revision, scenario results and aggregate hold/settlement
evidence. Keep rates, amounts, identities and credentials out of GitHub. These
checks do not establish enforcement for runtime requests outside this broker.
