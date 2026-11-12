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

Operations > Settings > Spend controls edits the request-model list using blank
model-name and framing-overhead inputs. The editor validates every entry and
refuses duplicate names. It remains unavailable until request-write availability
is explicitly true; an empty model list still permits foundation edits when
their own gate is open. A retained nonempty list requires both gates for every
settings save. Concurrent changes to the same list require loading the current
server list before reapplying the retained draft. Saving a list never activates
enforcement or certifies provider finality.

## Reader baseline before request policy saves

This extension has a separate strict deployment boolean,
`CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED`, false in code and ConfigMap.
An already-open foundation write gate does not authorize the new request-policy
shape. Until both write gates are enabled, any nonempty `request_models` save is
refused with HTTP 503 `model_budget_writes_unavailable`, including disabled drafts.
Empty request model lists are omitted from storage for the foundation reader.

First deploy this reader with request writes off. The release operator must
verify all serving readers and pin a compatible rollback image/digest for this
exact settings and reservation schema, including final request-count metadata.
Drain older writers before a separate fenced rollout enables the request write
flag. Closing either write gate retains compatible reads and existing liability;
it does not restore an earlier reader's compatibility. Preserve every hold and
stop record when rolling back. Production changes remain release-operator work.

Admin settings responses expose read-only
`model_request_budget_writes_available`, true only when both gates permit saves.
The UI must treat false or missing as saving request model bounds not yet
available. This capability cannot be supplied in a PUT/PATCH body. Revision CAS,
authorization, validation and confidential audit apply after enablement. Budget
enforcement activation still requires separate private owner/lead direction.

## Certified request bounds

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
final main-turn usage is unavailable. The parent debit consumes the turn allowance
too; observations alone cannot create capacity for auxiliary calls. The current
runtime source never certifies final usage or final request count, so enabled
auxiliary calls conservatively refuse until the authoritative gateway integration
supplies both. Success tests explicitly seed that future settlement contract;
they do not demonstrate current production settlement completeness.

`max_calls_per_turn` includes the certified parent request count and every granted
auxiliary permit. Concurrent holds serialize the count, and neither settlement
nor restart refunds it. Missing parent call-count evidence fails closed. Lowered
current limits and the original turn limits both constrain new requests.

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

Complete successful provider responses with the exact model and both usage counters settle the
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
