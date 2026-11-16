ADMIN contribution for #2149. Developer01 owns ADM040–057, ADM070–078,
ADM080–085 and ADM090–095. Developer09 owns the other 37 ADMIN rows.
Developer15 owns the shared catalog, registry, adapters and integration branch.

All 39 assigned rows now have exported scenario implementations. They have not
been executed against the real two-AM image and are not runtime acceptance.
Missing authentic fixtures produce BLOCKED before any request. Unimplemented
rows outside this assignment are owned by their catalog contributors.
Node contract tests exercise evidence, refusal and cleanup behavior only.
The sole parent supplies the compiled browser, two-AM image, role credentials,
owner attribution, restart and repetitions; this directory starts no processes.

Every `admin-fixtures.take(name, ctx)` returns `scope: LOCAL_DISPOSABLE_TWO_AM`,
`createdByRun: true`, and `cleanup()`. Setup must register its own resources
before returning; cleanup handles partial setup and uncertain public requests.
No setup returns an assertion result or executes a checklist row. All HTTP
operations, including any fixture setup/probes, use the supplied context's
request/evidence facilities. Credentials never enter returned data or artifacts.
Global settings/admission fixtures hold the parent's exclusive lease until
cleanup; returning a boolean without actually acquiring the lease is invalid.

`setup-requirements.json` maps all 39 rows to 37 named setups. One concrete public
setup (`settings_retention`) is included in `fixture-provider.mjs`; the other
36 require authentic factories from the sole parent. They are not implemented
by a boolean declaration or synthetic successful response. Do not advertise
all 39 cases as executable merely because an `admin-fixtures` key exists.

Create `provider = createAdminFixtures({config, adapter, withExclusiveSettings, factories})`
and install `{'admin-fixtures': provider, ...provider.capabilities}` in the parent's
`services.executable.fixtures`. Each case requires its exact `admin-setup-*`
factory capability as well as the generic adapter, so a generic key cannot
inflate executable counts. Config binds exact
source/image, two loopback AM origins, disposable database and parent-only
restart authority. `adapter.topology()` must observe both actual Ready members.
The adapter holds the parent's callback lease until final case cleanup, retains
live topology evidence, and refuses absent factories before taking the lease.
Factories receive `ctx`, use its real request services, register partial setup
custody immediately, and return raw data, bounded fault primitives and cleanup.

First-slice fixture contracts (the full inventory is in setup-requirements.json):

| Setup name | Required setup data and primitive controls |
| --- | --- |
| settings_retention | exclusiveSettings; writable retention; settings restored by this contribution |
| settings_read | exclusiveSettings; current retained spend policy with write gate off; `faults.failNextRead({path})` returns cleanup; `withRole` for isolated read_operator |
| settings_conflict | exclusiveSettings; writable settings; `faults.withholdNextSuccessfulResponse({method,path})` forwards exactly once then withholds ACK; handle returns cleanup/receipt; `concurrentLists(ctx)` returns valid initial/server/draft budgets and cleanup, not a verdict |
| settings_patch | exclusiveSettings; source-bound resetDefaults; patchSections covering all supported sections with actual changed valid values; withRole |
| settings_audit | matching actual actor; `useAuditCohort('no_current_revision')` selects a prepared bounded activity fixture without altering production data; withRole |
| idle_preview | exclusiveSettings; exact isolated cohort of idle/busy/Always-on Sandbox IDs, each `{id,kind}`; scanner is otherwise empty and no spontaneous maintenance during the preview |
| activity_full | immutable records and request receipts, actor/action; failed and successful configure/delete records; withRole; failNextRead |
| activity_table | at least 51 stable matching records in newest-first order, known actor/action labels, succeeded/refused/failed in first page and missing-name cases |
| admission_read | exclusiveAdmission; public-created history of at least 21 transitions; `setWriteAvailability(false)` selects a real prepared unavailable writer fixture; cleanup restores selection |
| admission_change | exclusiveAdmission; initially open writable control; freshCreate.take(am) returns unique original create body/key; custody(intent) registers public cleanup and unknown-result reconciliation before dispatch; sampleExistingWork returns real work_id/running/sequence/restarts; withholding fault handle |
| spend_read | selectMode(enabled/disabled/dependency_unavailable/enabled_without_cloud), expectedEnabled/expectedDisabled bound to actual local accounting inputs; withRole |
| spend_observation | enabled isolated cloud ledger, positive safe units and observedThrough; setWritesAvailable(false) selects the real prepared gate; withRole |

Role requests now use the exact named `ctx.request.actor` from the routing
contract at f5d2d08d3. ParentTransport must bind each named principal; missing
roles are BLOCKED and never fall back to admin. Agent mutations omit `am` by
default to select the observed non-owner. Explicit reads retain actual ingress
receipts. Browser mutations use observed non-owner routing; `switchAM` directs
subsequent reads, preserving browser identity.

Fault handles expose raw `receipt()` data: forwarded_writes, response_withheld,
and following_requests in actual dispatch order. They do not return PASS/FAIL.
Unavailable writer modes require actual local service configurations; a fake
response is not eligible feature evidence. If these modes cannot be provided by
the sole parent, their cases remain BLOCKED and are counted in the denominator.

Run offline checks with:

    node --test .github/scripts/multi-am-features/executable/cases/developer-01/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs
