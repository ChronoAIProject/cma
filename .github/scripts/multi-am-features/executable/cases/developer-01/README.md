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

`setup-requirements.json` maps all 39 rows to 37 named setups. Six concrete public
setups (`settings_retention`, `spend_period`, `settings_patch`, `settings_audit`,
`settings_conflict`, `settings_read`) are included; the other 31 require authentic factories from
the sole parent. They are not implemented
by a boolean declaration or synthetic successful response. Do not advertise
all 39 cases as executable merely because an `admin-fixtures` key exists.

Create `provider = createAdminFixtures({config, adapter, withExclusiveSettings, factories})`
and install `{'admin-fixtures': provider, ...provider.capabilities}` in the parent's
`services.executable.fixtures`. Each case requires its exact `admin-setup-*`
factory capability as well as the generic adapter, so a generic key cannot
inflate executable counts. The registry also loads `index.mjs::setups`; these
are dispatchers to the bound capabilities, never declarations that absent
recipes exist. The new `fixturesForCase` bridge replaces the generic adapter
but still enters the same lease/topology/cleanup wrapper. Config binds exact
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
| settings_read | built in: exclusiveSettings; actual retained spend policy with unavailable writer; distinct settings_operator/read_operator identities verified by public allow/deny reads; `faults.failNextRead({session,path})` aborts one browser GET and returns cleanup/receipt with real recovery responses |
| settings_conflict | built in: exclusiveSettings; writable settings with background policies disabled; `faults.withholdNextSuccessfulResponse({session,method,path})` forwards the first PUT once and withholds only its real 2xx response; handle retains cleanup/receipt; `concurrentLists(ctx)` returns valid initial/server/draft budgets and cleanup, not a verdict |
| settings_patch | built in: exact-source reset defaults, real published alternate Agent profile, eight changed sections, reset audit, settings restoration before profile retirement |
| settings_audit | built in: real retention patch and exact actor audit, then 25 public admission stop/resume pairs; settings revision unchanged while 50 newer real configure records fill the recent page; exact settings-only actor without activity permission |
| idle_preview | exclusiveSettings; exact isolated cohort of idle/busy/Always-on Sandbox IDs, each `{id,kind}`; scanner is otherwise empty and no spontaneous maintenance during the preview |
| activity_full | immutable records and request receipts, actor/action; failed and successful configure/delete records; exact named roles; failNextRead |
| activity_table | at least 51 stable matching records in newest-first order, known actor/action labels; the first page must include succeeded/refused/failed and an actual missing-name record, asserted separately from outcome examples |
| admission_read | exclusiveAdmission; public-created history of at least 21 transitions; `setWriteAvailability(false)` selects a real prepared unavailable writer fixture; cleanup restores selection |
| admission_change | exclusiveAdmission; initially open writable control; freshCreate.take(am) returns unique original create body/key; custody(intent) registers public cleanup and unknown-result reconciliation before dispatch; sampleExistingWork returns real work_id/running/sequence/restarts; withholding fault handle |
| spend_read | selectMode(enabled/disabled/dependency_unavailable/enabled_without_cloud), expectedEnabled/expectedDisabled bound to actual local accounting inputs; exact named roles |
| spend_observation | enabled isolated cloud ledger, positive safe units and observedThrough; setWritesAvailable(false) selects the real prepared gate; exact named roles |

Role requests now use the exact named `ctx.request.actor` from the routing
contract at f5d2d08d3. ParentTransport must bind each named principal; missing
roles are BLOCKED and never fall back to admin. Agent mutations omit `am` by
default to select the observed non-owner. Explicit reads retain actual ingress
receipts. Browser mutations use observed non-owner routing; `switchAM` directs
subsequent reads, preserving browser identity.

Fault handles expose raw `receipt()` data: forwarded_writes, response_withheld,
response_status and following_requests in actual dispatch order. They do not return PASS/FAIL.
Unavailable writer modes require actual local service configurations; a fake
response is not eligible feature evidence. If these modes cannot be provided by
the sole parent, their cases remain BLOCKED and are counted in the denominator.

The conflict recipe reads actual public settings and requires disabled auto-pause,
triggers and spend enforcement plus compatible writers for retained policies.
It supplies distinct synthetic owner-policy inputs without setup writes. The case
performs the competing writes and restores the original policy. Its response fault
uses the parent's real Playwright page and exact loopback proxy/settings path;
the shared proxy still records the actual AM request. Forwarding has no retries,
no redirect following and a 10-second timeout. A non-2xx response reaches the UI
unchanged. Unknown forwarding errors fail cleanup and are never replayed.
The fault tracks later requests until the UI loads current values, then removes
the route and drains in-flight work before retaining evidence. The case requires
one successful PUT, a withheld response, a subsequent GET and no extra mutations.
Only then does it begin the intentionally conflicting list writes. No request
bodies, credentials or synthetic successful responses enter the fault receipt.

The read recipe verifies real settings access for the configured settings operator,
denial for the read-only operator, and the unavailable writer state before opening
the browser. It does not alter writer availability or create retained policies;
missing actual conditions remain BLOCKED. Its fault aborts one exact local settings
GET. The failed attempt records a browser transport failure with no HTTP status or
serving AM. Later reads continue through the parent's proxy, including the explicit
read through the other AM. The case verifies that the prior draft remains visible,
the recovery GET returns 200 and no settings mutation is attempted. Fault timing
ends at abort completion or response headers; complete request and UI timings remain
with the shared adapter and runner. Cleanup removes both listeners and the route,
joins the abort, and preserves errors. Public reads still leave immutable audit
records for parent database disposal. The case repeats its read-only browser
assertion after browser close/drain, so a late mutation also prevents success.

The patch/reset recipe reads the five immutable source files at the exact
parent image source from Git objects and checks their hashes against
`reset-contract.json` before using the reviewed public reset defaults. A parent
running an archived coordinator must bind `config.admin_source_repository` to
an absolute, read-only local checkout containing that exact source commit.
Without an explicit binding, a repository-backed coordinator uses its own root;
an archive blocks. The recipe never fetches, checks out, or reads working-tree
files. Invalid explicit bindings cannot fall back to another repository, and
cached verification is isolated by canonical repository path and source commit.
The `admin_reset_source_contract` observation retains the commit and five file
hashes, without the local repository path. Unavailable or changed source blocks
before public setup requests or profile creation. Auto-pause,
trigger dispatch and spend enforcement must already be disabled in this
isolated policy fixture; the recipe never disables an active production policy.
It creates and publishes one disposable Agent profile through public APIs to
exercise a valid changed default. Cleanup restores the settings before retiring
that profile. Lost create/retire acknowledgements are never replayed.

The audit recipe requires exact `config.actors.admin.subject` and
`config.actors.settings_operator.subject` bindings before changing anything.
The provider holds the parent's global lease throughout setup, the browser
case and cleanup. Auto-pause, triggers and spend enforcement must already be
disabled, and admission must be open with its write gate enabled. The setup
changes retention once to establish a real settings audit. It then uses 25
admission stop/resume pairs, alternating AM ingress, to produce 50 newer audit
records without changing settings or allocating a runtime. The API page must
contain exactly those unique successful admission revisions; a clock delay alone
does not prove displacement. Admission is left open, settings are restored, and
the parent owns final disposal of the immutable history/audit records. Uncertain
stop/resume acknowledgements are read back and never replayed. An unconfirmed
resume or foreign admission change prevents cleanup success. The case still
requires the real compiled UI and cookie identity for both admin roles.

All adapter-backed cases require the parent's final database disposal receipt:
public reads/writes leave immutable audit rows and retired profile records.
Installing a recipe is not runtime readiness. The parent must also set exact
`services.executable.readyCaseIds` after authentic bindings, or an actual
executed response must establish executability under the shared count contract.

Admission stop/resume cases retain resume intent before dispatch, including the
ordinary resume and the deliberately withheld acknowledgement. Cleanup reads
current admission state first. A committed open state can be confirmed by reads;
an unchanged stop after an unknown resume stays unresolved and is never resumed
again. Explicit restoration and deferred cleanup share one promise, so a lost
cleanup acknowledgement cannot trigger a second dispatch or erase its failure.
Other resource cleanup callbacks still run and foreign stops remain protected.

Owned create helpers retain an optional parent `capture(response)` receipt after
the request, including an accepted response attached to an attribution failure.
Capture errors preserve the original request failure and never replay the create.
The three quota creates all settle before failure propagates to cleanup, so a
late accepted resource cannot escape while cleanup is already running.

ADM-045 has an optional concrete parent-bound recipe exported as
`user-limit-setup.mjs::createUserLimitFactory({userResources})`. Install its result
as `services.executable.adminFactories.user_limit` (or the provider's
`factories.user_limit`). It is not a seventh automatically installed public recipe;
all 31 parent-bound setups still require authentic bindings. The recipe reads
writable settings with disabled autonomous policies and verifies empty owner
inventory through both AMs before preparing three distinct pinned-profile creates.
It does not create resources during setup. The case changes the limit to two,
dispatches three creates concurrently, checks exactly two 201 responses and one
typed quota refusal, then joins both inventories to the exact accepted IDs.

Bind distinct actual `config.actors.user.subject` and `config.actors.admin.subject`.
The parent must supply a genuinely empty disposable user for each repetition.
Historical retired rows are included, so reusing a nonempty account blocks;
the recipe neither deletes historical records nor hides them with a query flag.
The original limit must differ from two so the case exercises a real policy change.

`userResources` reuses the resource-custody inputs in developer-17:
`scope: LOCAL_DISPOSABLE_TWO_AM`, `database_disposable: true`,
`single_matrix_writer: true`, a pinned `sandbox_profile: {id, revision}`,
`reserve({purpose, caseId})` and actual independent `finality({sandboxId})`.
An optional `timeout_ms` is bounded to 2000–300000 ms. The reserve is subordinate
resource custody; it must not reacquire the global settings lease already held
by `createAdminFixtures`. The returned lease needs `parent_cleanup_barrier: true`,
`retainIntent`, `retainResource`, `release`, and the quota-specific
`retainRefusal({key, actor, am, response})`. That last method durably resolves only
the original typed 429 receipt; it is not an outcome oracle. Full response timing
and serving evidence are retained privately, including the original request ID.

Intent retention precedes each POST. Accepted IDs enter local custody before the
parent evidence write. Lost acknowledgements, failed evidence writes, malformed
successes and unmatched refusals remain unresolved without replay or rekeying.
Cleanup joins one attempt, verifies exact ID/owner/name/profile, and issues at
most one public retirement per resource. It continues other roots after failures,
requires the public retired state plus independent zero-child/zero-pending native
finality, and releases custody only after all known outcomes are resolved. Lost
retirement acknowledgement stays a cleanup failure even if a later read confirms
retirement. Final parent database disposal is still required for immutable records.

ADM-080 also has a parent-bound recipe:
`cross-read-setup.mjs::createCrossOwnerReadFactory(bindings)`, installed as
`adminFactories.cross_owner_reads`. It reuses developer-17's `resources` helper
to create an actual disposable Sandbox, inline Workspace and ready Agent via
public APIs. Bind its existing `userResources` custody/profile/native-finality
contract plus `identities.user`. `config.actors.user` and `config.actors.creator`
must explicitly name that same subject; `read_operator` and `outsider` must name
two different subjects. Missing or mismatched roles block without borrowing
credentials. The read operator must have actual foreign-owner view/history/files
authority and no interaction, settings, approval or sharing authority. The new
resource has no sharing grant; the outsider must receive a real denial.

After the existing hierarchy helper confirms its initial turn, the recipe sends
one uniquely marked prompt, waits for its completed canonical assistant message,
and writes a uniquely named text file through the creator API. It retains each
intent before dispatch and verifies the file bytes through the opposite AM from
the actual write receipt. Provider/model/runtime availability comes from the
parent; no output is synthesized. Unknown input/file acknowledgements fail setup
without replay. The already-owned disposable Sandbox contains all created data,
and public retirement plus actual native finality owns cleanup on every path.

The ADM-080 browser case opens Agent computer → Files, selects the exact path,
and downloads through each AM. Each browser response's request ID must join the
corresponding shared serving receipt; requested AM labels alone do not qualify.
Filename and bytes are checked, and full serving evidence is retained. Browser
read-only checks repeat after close/drain so late mutation receipts prevent PASS.
This recipe still requires parent selection and real compiled cookie sessions;
six automatic recipes / 31 parent-required setups and runtime NOT_RUN are unchanged.

Run offline checks with:

    node --test .github/scripts/multi-am-features/executable/cases/developer-01/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs
