# ADM-055 default owner policy

The `default_owner_policy` case starts with enabled metering, a valid platform
policy, one owner with an explicit override, and a distinct owner without an
override. The parent supplies real reservations and authenticated admission
receipts. An absent default policy is a required starting state; the case does
not manufacture an owner or infer a reservation from the settings response.

The UI fills every default-policy control: daily and turn caps, maximum calls,
IANA day timezone, input and output valuation, unobserved allowance and usage
timeout. It joins each full settings `PUT` to the exact expected revision,
body, response and request ID. The peer AM must read the complete changed
policy. Raw browser evidence retains serving, post-serving, timing and
attribution fields, and the serving AM must match the selected ingress AM.

Admission is checked through two real owner identities. The owner without an
override receives the newly saved default policy and one provider effect; the
owner with an explicit override receives its distinct policy and one provider
effect. Each admission must retain a concrete request identity, configured
model, local scope, reservation ID and provider journal receipt linked by
reservation ID. A reported counter or echoed policy is insufficient. Removing
the default is a separate exact save. The formerly unscoped owner is checked
through the model-request admission surface: the fixture must expose the
source-backed `BudgetError::Unavailable`/`budget_unavailable` refusal with the
owner, model, scope, request identity and null reservation joined to the raw
observation. The case does not manufacture an HTTP status for an internal
caller. Existing reservations remain byte-for-byte present. This proves
default scope and retention instead of treating a successful settings read as
enforcement evidence.

The browser is closed and drained before final evidence and the memoized
settings restoration. Every mutation request, body and response is captured;
only a proven operations report admission may be filtered from the mutation
guard. Deferred cleanup joins the same finality operation, and close, evidence,
response-body, ingress, admission and reservation failures remain visible. The
reservation inventory is re-read after close/drain, including failed cleanup.
The explicit removal returns the policy to its original state, so restoration
correctly performs no additional write. The offline model starts
no application, browser, native process or database; authentic parent fixtures
and the selected two-AM campaign remain required for runtime acceptance.
