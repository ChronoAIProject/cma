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
and downloads through each AM. It reads the computer toggle's expanded state
before clicking, preserving the automatically open desktop panel while opening
the collapsed narrow layout. Each browser response's request ID must join the
corresponding shared serving receipt; requested AM labels alone do not qualify.
Filename and bytes are checked, and full serving evidence is retained. Browser
read-only checks repeat after close/drain so late mutation receipts prevent PASS.
This recipe still requires parent selection and real compiled cookie sessions;
six automatic recipes / 31 parent-required setups and runtime NOT_RUN are unchanged.

ADM-085 requires each parent `deniedOperations` entry to carry a source-derived
`denials: [{status, code}]` contract. Only exact 403/404 plus typed problem-code
pairs qualify. A generic 404, an unrelated error code or a mismatched pair fails;
an absent contract blocks before any case request. A missing route is not proof
of a custody boundary. The parent must bind documented current sharing, service,
bridge-management and profile operations against real owned fixtures, plus public
before/after state and the positive owner control. This source tree exposes AM
channel-usage history under `manage_bridge`; that read-only surface does not
replace the required bridge-management mutation. The corresponding authentic
management binding remains outstanding, and this change installs no new factory.

ADM-082 has a parent-bound recipe exported as
`cross-controls-setup.mjs::createCrossOwnerControlsFactory(bindings)`, installed
as `adminFactories.cross_owner_controls`. It reuses the public hierarchy and
native custody helper with `userResources`, plus `compiledWeb` and
`identities.user/control_user/control_operator`. All three identities must match
their exact configured subjects. `user` and `creator` must explicitly alias the
same subject; the creator, two control identities and read operator are distinct.
The recipe reads actual foreign-owner resource access and authenticated cookie
session capabilities: both control identities need `control_any_sandbox`, and
only `control_user` has `use_own_sandboxes`. Compiled build identity is verified
through the existing browser helper. These checks do not grant missing roles.

The public event head must show no active turn, settings must be settled and
preferences must support system prompts. The recipe changes no global policy.
It supplies distinct settings, prompt, title, file, attachment and command keys;
the case performs the control mutations. Exact file bytes, settings and
preferences are read through the AM opposite their actual write receipts.
Shared Rename may be absent or disabled; the owner's positive control must be
visible and enabled after prompt settlement. The file and configuration restores
are one attempt each, shared with deferred cleanup. Lost acknowledgements stay
failed even if the mutation committed; cleanup never dispatches them again.
Parent retirement and independent native finality remove the attachment and
remaining disposable hierarchy; immutable records require database disposal.

The original ADM-082 control-only preference GET denial remains asserted.
Current `agent_reads/configuration.rs` authorizes that read with `operate_agent`
and does not require `use_own_sandboxes`; the write path does require both.
Setup checks role capabilities rather than requiring the expected denial, so
this difference yields a case failure instead of a fabricated PASS or a setup
block. Source intent versus checklist expectation requires review and actual
runtime evidence. This recipe adds no automatic readiness; six automatic public
recipes and 31 parent-required setups remain.

ADM-050 has a parent-bound recipe exported as
`defaults-marketplace-setup.mjs::createDefaultsMarketplaceFactory(bindings)`,
installed as `adminFactories.defaults_marketplace`. Bind the existing
`userResources` contract (disposable database, sole matrix writer, subordinate
resource lease and independent native finality) and
`defaultsCapacity: {cpu_millis, memory_mib}` from the actual parent native lane.
The capacity must fit the public authoring bounds. `config.actors.user` and
`admin` must be distinct exact subjects. The user needs real public profile
creation/publication/visibility and Sandbox/Workspace/Agent creation authority.
Use a genuinely empty disposable owner for each repetition: both AM inventories,
including retired rows, must be empty. The parent already holds the global
settings lease; automatic pause, triggers and model budgets must be disabled.

The recipe creates, publishes and reads back three unique profiles using public
APIs. They contain no services, scripts, skills, environment values or nested
profile provisioning. Their immutable revisions become the tested defaults.
The case issues exactly one `POST /api/v1/conversations` with `{}`: this source
path uses all three platform defaults, whereas `POST /api/v1/agents` creates
inline profiles. The returned conversation metadata retains Sandbox/Agent
custody; Agent detail is read with its actual Agent attribution through the AM
opposite the observed creation receipt. The catalog read uses the unique public
Agent-profile name and verifies the changed marketplace policy.

The conversation route has no idempotent replay contract. A transport failure
without resource identity retains unresolved parent custody; an accepted response
followed by attribution/evidence failure retains its original IDs for cleanup.
No uncertain create or retirement is retried. Cleanup verifies exact Sandbox
owner/profile, waits for public retirement and independent native finality,
restores original settings, and retires all three profiles before the fixture
wrapper releases the settings lease. Repeated hooks join the same attempt and
preserve failures. Parent database disposal owns immutable profile/audit records.

The original ADM-050 unavailable-reference refusal remains asserted. Current
`PlatformSettings::validate` rejects revision zero, but the settings service and
store do not resolve positive default-profile references before saving them.
An accepted unpublished revision therefore remains a case failure, with its
negative-test intent retained so cleanup can restore original settings. This is
a source finding, not a runtime verdict or a change to the checklist. The recipe
requires parent binding and selection; six automatic / 31 parent-required
setups and runtime NOT_RUN are unchanged.

ADM-046 now edits Visitor Sandboxes and Visitor launches per day through the
compiled settings UI using the exact admin cookie session. Its one successful
PUT is joined by response request ID to the retained browser serving receipt;
readback uses the opposite actual AM and verifies the full desired settings.
The browser closes and drains before the fresh-CAS restoration. Deferred checks
retain late extra mutations, close failures and evidence failures independently.
A bearer-only browser, unchanged policy or unjoined save cannot qualify.

The parent `visitor_limits` factory still supplies the actual unprivileged
visitor and isolated public resources. It must return two POST Sandbox intents
at `/api/v1/sandboxes`, three POST public Trigger Launch intents at
`/api/v1/triggers/public/{public_id}/launches`, and five distinct original keys.
Each `custody(intent, {actor, am})` returns `capture(response)` and memoized
`cleanup()` before dispatch; cleanup is registered even if capture is missing.
The capture hook retains original complete 201/202/429 receipts, including an
accepted response attached to a later attribution error. Unknown outcomes stay
under parent custody; the case does not retry, rekey or start later intents after
failure. Effective visitor identity is explicit and cannot fall back to admin.

Both-AM evidence comes from actual response receipts, retained without projecting
away serving/timing fields. Public Sandbox creation must return 201; Launch
creation must return 201 or 202. Accepted IDs must be present and unique, and
the independent `readPublicInventory(ctx)` result must match those exact IDs,
not only the expected counts. The Launch refusal must identify the daily visitor
Launch quota with limit 2 and a valid reset timestamp. The parent owns safe public
Launch cancellation/root retirement, native finality, quota isolation and final
database disposal. This remains an unbound parent factory; six automatic public
recipes / 31 parent-required setups and runtime NOT_RUN are unchanged.

ADM-048's read-only proof retains the actual admin Sandbox `lifecycle` and
`work` state, status, Always-on preference and complete event history. The API
has no top-level `lifecycle_operation` field. Event reads use the documented
`after` cursor with 200 records per page; IDs and contiguous sequences must
match, and a short page proves completion. Twenty full pages hit an explicit
failure bound; truncated history cannot qualify. The parent supplies an isolated
idle/busy/Always-on cohort whose lifecycle/provision state stays quiescent during
this comparison, apart from the held busy Agent work. Distinct IDs are required.

The compiled admin cookie UI preview response is joined by request ID to its
actual selected-AM serving receipt, and both its body and displayed count are
checked. The browser closes and drains before the final lifecycle snapshot and
read-only evidence check. Every POST/PUT/DELETE or other mutation is refused as
preview evidence, including late requests observed during shutdown. Close,
evidence and lifecycle failures are collected independently, and repeated cleanup
joins the same failed attempt. Full private serving receipts and before/after
snapshots remain available. The actual cohort, role and scanner isolation are
still authentic parent dependencies; this change adds no factory or runtime PASS.

ADM-049 requires the same isolated scanner cohort and real compiled cookie UI.
All cohort IDs are distinct, each Sandbox starts active with no lifecycle
operation, and only the Always-on records have that policy enabled. The parent
holds background maintenance and lifecycle changes quiescent; this is an actual
fixture obligation, not an executable-readiness boolean. The unconfirmed request
must return the typed 400 and leave the full lifecycle/work/event snapshot
unchanged when read through the opposite AM.

The case accepts only the exact compiled confirmation dialog and verifies that
the UI submitted `{confirmed: true}`. Its single response must be HTTP 200 with
a request ID joined to the actual browser serving receipt. All three scheduling
counts are nonnegative integers. Because this isolated cohort starts active and
unscheduled, scheduled equals the idle count, already_satisfied is zero, and
skipped equals the busy plus Always-on count. The displayed acknowledgement is
checked without treating it as completion. The browser closes and drains before
polling both AMs for paused idle records; final snapshots also require no pending
idle lifecycle operation and unchanged protected Sandbox state/events.

`idle_pause.readBusyWork(ctx, sandbox)` returns raw observations for each busy
Sandbox: `sandbox_id`, `agent_id`, positive `turn_sequence`, stable `runtime_id`,
nonnegative progress `sequence`, and native `restarts`. These fields come from
the original held turn and its native runtime, not synthesized fixture results.
They are validated before dispatch. Final progress must increase on that exact
Sandbox/Agent/turn/runtime with zero restarts. A different work identity with a
higher sequence does not qualify. Full observations and serving fields stay
private and unprojected. Close, evidence, protected-state and work-read failures
are retained independently; repeated cleanup joins the same attempt. Unknown
acknowledgements never cause another confirmed click. Parent public revival,
held-turn cleanup, native finality and database disposal remain required. There
is no new automatic setup or runtime acceptance claim.

ADM-047 now follows the public Launch API. `LaunchView` has `id`, `state`,
`blocked_reason` and `resolution`; it has no `occurrence_id`. Trigger enablement
controls new admission, whereas orchestration pause blocks existing admitted
work at the worker boundary. The case tests those distinct behaviors rather
than treating an existing Launch as a future scheduled occurrence.

The authentic `trigger_settings` factory supplies `triggerId`, `publicId`, two
`disabledIntents` and one `launchIntent`, with three distinct original keys.
Each intent is a user POST to `/api/v1/triggers/public/{publicId}/launches` with
`{start_if_ready: true, choice: 'new'}`. Public reads must prove an owned published
Trigger, no publisher review, no required services, an empty first message and
an initially empty user Launch inventory for that Trigger. The parent supplies
real publication and credentials; the case creates no fixture verdict.

Compiled cookie settings saves have exact response-ID/serving joins and fresh
opposite-AM readback. While disabled, each AM must return typed 503
`triggers_disabled` for its own original intent and the inventory stays empty.
While enabled and paused, one Launch is admitted. Both AMs must expose that
exact immutable Launch/revision identity blocked as `orchestration_paused`, with
all three resources still pending. After the UI resumes orchestration, the
browser closes/drains and both AMs must report the same Launch Ready/completed,
with a valid ready timestamp and matching concrete resource identities. The
complete per-trigger inventory remains exactly that one ID. A nonterminal
acknowledgement, different block, duplicate Launch or resource disagreement fails.

Every intent obtains parent `custody(intent, {actor, am})` with `capture(response)`
and memoized cleanup before dispatch. Full accepted/refused responses are retained,
including the original response attached to an attribution error. Other errors
must retain unknown outcome custody; they do not authorize a retry. Browser
close/drain precedes newer resource cleanup hooks and fresh-CAS policy restoration.
The parent owns exact public Launch cancellation/root cleanup, native finality
and database disposal. This adds no automatic factory: six automatic recipes /
31 parent-required setups remain.

ADM-075/076 also use actual public Launch state. Their `trigger_commands`
factory supplies the same published/no-review/no-services/empty-first-message
Trigger and one original user `launchIntent`, plus `adminSubject` taken from the
parent-bound admin credential identity. There are no disabled intents or
synthetic occurrence/scheduler hooks. The parent global lease remains held
through all cleanup. Before admission the per-trigger user inventory must be
empty through both AMs.

For pause, the dedicated command first changes a running policy to paused, then
one admitted Launch must remain blocked with all resources pending. For resume,
one Launch is first observed paused on both AMs; the dedicated command must
advance that same Launch to Ready/completed with matching resource identities.
The whole returned settings snapshot, advanced revision and actual command
request/serving receipt are retained, with exact both-AM readback. A lost command
acknowledgement never causes another POST; any original response attached to an
attribution failure is captured without discarding either error.

Audit correlation uses the fields the API actually exposes: exact actor,
configure action, resource target `triggers`, command revision, succeeded outcome
and paused/running detail. It does not invent an audit request-ID field. Complete
actor-filtered pages are read separately through both AMs, at most twenty pages
of fifty. Missing, duplicate, repeated-cursor or truncated evidence fails; the
matching audit must have a fresh ID absent before the command, and both AMs must
return the same audit ID/timestamp. Full records remain private.

The case and runner join the same original resource-cleanup attempt. A paused
Launch must be publicly cancelled, with its original identity and pending
resources read back, before policy restoration can resume work. Completed Launch
cleanup retires the created roots; Ready Launch records cannot be cancelled by
this API. Native finality remains the parent's obligation. Fresh-CAS restoration
and independent cleanup failures are retained; final audit/settings/inventory
checks cannot substitute for native disposal. These rows add no automatic
factory or runtime acceptance claim.

Run offline checks with:

    node --test .github/scripts/multi-am-features/executable/cases/developer-01/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs

ADM-042 requires an isolated idle/busy/Always-on cohort with distinct `{id,kind}`
entries, exclusive settings custody, and auto-pause initially disabled. Its parent
`readBusyWork(ctx,sandbox)` returns the original `sandbox_id`, `agent_id`,
`turn_sequence`, `runtime_id`, progress `sequence` and `restarts` through authentic
observations. The same busy turn must progress without restarting; protected
Sandbox lifecycle/work snapshots and complete event histories must remain equal.
The parent publicly revives/retires test Sandboxes and proves native finality
before releasing custody. This remains one of the 31 parent-required factories.

The case no longer consumes `waitIdleThreshold` or any elapsed-grace verdict.
After real cookie UI saves at both inclusive bounds (60/86400) and typed 422
refusals outside them, it enables the one-minute policy via hours/minutes/seconds.
Every save joins its exact body/revision, response, request ID and actual serving
AM; both AMs must return the same complete policy including server-owned grace.
It closes/drains the browser, then measures a full monotonic minute after enable
readback, preserving server grace and local start/end times. Wall-clock jumps
cannot shorten this wait. The runner's default 60000-ms budget is insufficient:
set `executable_case_budget_ms` with setup/scanner headroom (for example 180000).
The case requires over 90000 ms remaining before setup and enable, reserves
30000 ms beyond grace, and bounds subsequent polling with 15000 ms for finality.
Missing time remains BLOCKED; no fixture clock or fast-forward is accepted.

Finality closes/drains before both-AM terminal lifecycle reads, original busy
work checks and one fresh-CAS restoration. Independent errors all survive; an
unknown save or restoration is never replayed. Cleanup uses checks valid outside
an active checklist step, and its failure remains sticky through deferred joins.

ADM-043's authentic report/cookie scope and cleanup contract is documented in
[measured-visibility.md](measured-visibility.md). It replaces fixture verdicts
with completed-report, inventory, rendered-allocation and exact null-gap evidence.
