# ADM-053 spend control availability

The `spend_availability` recipe exercises the compiled Operations settings UI
with the parent-owned writer gate. It starts from a readable, writable policy
that contains the retained platform period, optional cloud policy, owner
overrides and request models. The case records both AM snapshots before the
toggle and after each gate transition; a repeated request is not evidence of
two-AM coverage.

Closing the model-budget writer also makes the public request-budget and
platform-spend availability flags false: both service getters include the
model-budget gate. The parent preserves the other configured writer gates,
while all three effective public flags are checked during the closed phase.
Recovery must restore their original values on both AMs.

The case first changes the enable control locally, records that intent, closes
the writer and attempts one full settings `PUT`. The UI must retain every
draft field, show the typed `model_budget_writes_unavailable` refusal, keep the
save action disabled, and leave the durable policy unchanged. A fresh settings
read is required before `Review retained draft` can re-enable the save action.
After the parent reopens the writer, the case verifies the exact revisioned
full body, response, request ID and serving AM. The peer AM must observe the
same policy and all availability flags.

The strict ceiling paragraph is captured through the checkbox's
`aria-describedby` link and exact visible text. It remains a product statement
about current limits, not a claim that a provider-spend ceiling exists. All
platform, owner and request-model controls are read back with their original
values, and list counts are checked against the durable response.

Normal recovery reopens the writer for the successful UI save. Finalization
closes and drains the browser, joins that same memoized recovery (or attempts
it once if an earlier failure prevented recovery), then restores settings once.
Deferred cleanup joins those same operations;
an uncertain refusal, save, recovery or restoration is retained as an unknown
outcome and never replayed. Close, browser evidence and both final AM reads
retain independent failures. Report prefetch `POST` admissions are captured
separately and cannot authorize an additional settings mutation.

The offline model mirrors the state machine and wire receipts without starting
an application, browser, native process or database. Its deliberate-fault
options cover wrong status/code/body, wrong serving attribution, duplicate or
unresolved mutations, lost drafts, stale peers, list mismatches, malformed
scope text, uncertain outcomes and cleanup failures. Runtime acceptance still
requires the parent's authentic fixture and the selected two-AM campaign.
