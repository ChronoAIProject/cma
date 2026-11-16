# Operations report admissions in ADMIN cases

Operations can prefetch an admin usage report while its settings tab is shown.
When the retained-view endpoint is unavailable, the source-defined fallback
submits a report POST. This is independent of the tested settings or lifecycle
action. Counting every browser POST/PUT as that action incorrectly fails an
otherwise valid case. It must also never allow an extra feature mutation.

`operations-prefetch.mjs` is the shared capture for ADM-040, 042, 043, 046,
047, 048 and 049. It attaches before navigation to the actual compiled browser
Page. Only requests at the exact canonical admin report-creation route enter
the report check, including the Web's `/api/agent-manager/` alias. They must
be POSTs with a resolved HTTP 200/ready or 202/pending admission, a nonempty
report ID, and one matching request-ID/method/status/actual-AM receipt. An
unmatched, duplicate, malformed or wrong-AM receipt fails. A pending request
may finish later, but must resolve by final browser drain. Response body work
is joined even when the adapter's evidence collection yields new responses.

The original browser evidence is retained privately through `ctx.observe`,
including report requests and complete serving snapshots, timing and attribution.
An additional private observation holds report request bodies and admission
identities. Only the collection passed to each case's feature-mutation guard
excludes proven report admissions. Settings writes, pause actions, unknown
POSTs and unrelated mutations retain their existing guards. A valid report
admission does not imply report completion or a usage performance PASS.

Callers close/drain before final capture. Listeners are removed during that
final capture even if adapter evidence fails. Capture and state restoration
errors remain independent and sticky through memoized cleanup. Checks reached
from cleanup run outside checklist steps; actual step wrappers still require
assertions after successful finality. Negative tests exercise this distinction.

The same authentic parent browser and settings/cohort fixtures are required;
the six automatic and 31 parent-required setups are unchanged. The parent
disposes immutable report/audit records along with the local database and owns
native resource finality. Importing this helper or executing its offline models
starts no browser, application or production resources. ADM-044 independently
retains its previously qualified retention-specific capture. No current runtime
selection or checklist acceptance is changed by source qualification.
