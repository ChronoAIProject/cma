# ADM-044 retention evidence

The existing `settings_retention` public recipe requires the sole parent's
exclusive settings lease, disposable database and authenticated compiled admin
cookie browser. It reads real policy; the case performs the mutations. The
recipe count remains six automatic and 31 requiring parent setup. Missing
bindings remain BLOCKED. Offline regressions do not qualify runtime acceptance.

The case opens Operations settings and checks the real retention control's
unsigned integer contract: required number, minimum 0, maximum 4294967295,
step 1. Both inclusive limits must satisfy DOM validity, but are not saved.
The actual save uses 30 days, or 31 if the original policy is already 30. Its
exact PUT body and revision join the browser response, request ID and actual
serving AM. Both AMs must return the same complete acknowledged policy.

Negative UI values (-1, fractional, overflow and empty) must fail DOM validation
and submit no settings mutation, even a mutation that the server rejects. The
case then closes/drains the browser before submitting direct malformed PATCH
values through the public API. Those requests must return HTTP 422 with code
`invalid_request`; both AMs must retain the complete original saved snapshot,
including its revision. The changed policy must preserve unrelated settings.

Operations may prefetch the default usage report while showing settings. The
capture distinguishes its exact POST route from retention PUTs. Report creation
must resolve to 200/ready or 202/pending with a nonempty report ID and an exact
ingress receipt. An in-flight prefetch may complete later, but it must resolve
by final drain. Other mutations, extra retention writes, duplicate/unmatched
receipts or unresolved outcomes fail. Full `serving`, `serving_after`,
`started_at`, `ended_at` and `attribution` fields remain unprojected in private
browser evidence. Report/admission status is separate from report completion;
ADM-044 makes no usage performance or report completion claim.

Listeners remain attached through browser close and drain; outstanding response
bodies are joined before final capture. Restoration starts afterward, uses a
fresh opposite-AM revision CAS, and only restores a policy whose intent this
case recorded. It is memoized, including unknown outcomes. Each cleanup hook
joins the original attempt; neither a save nor a restoration is replayed after
an uncertain acknowledgement. Browser, evidence and restoration failures survive
independently, including cleanup outside a checklist step. Parent disposal owns
the immutable settings/read audits and any prefetched report records.

The source prefetch finding also applies to other Operations-based cases. Their
existing mutation guards need separate review; this change does not certify or
alter those cases. No application, browser, native fixture or production action
is started by importing the case or running its offline model tests.
