ADM-043 runtime fixture contract (#2149)

`measured_visibility` remains a parent-required factory. It holds the real global
settings lease and returns local disposable setup data, public/native cleanup,
`range: {from,to}` as integer Unix seconds, and
`scopes: {user: {subject,sandboxIds}, admin: {subject,sandboxIds}}`. Both actors
must have distinct actual cookie identities and their own authorized report scope.
Each scope contains 1–50 unique known Sandbox IDs; the complete isolated inventory
must fit the first UI page. The range is fixed, historical, nonempty and at most
30 days. The parent keeps allocation inputs stable across the two visibility
modes, supplies authentic missing CPU measurements, and owns immutable report,
audit and allocation-history disposal. No completedUsage boolean, expected PASS,
synthetic report response or measurementGapMarker is accepted.

The case toggles the real settings checkbox twice, retaining the intent before
save. Exact PUT body/revision, successful response and actual serving AM join
through request ID. Both AMs must expose the complete saved policy. Each mode
opens fresh personal and admin browsers on the opposite AM. The custom historical
range prevents unrelated 7d/30d warming from becoming selected report evidence.

A committed dashboard and rendered inventory must join an actual ready,
report_complete descriptor and the matching complete Sandbox page. The principal,
admin scope, range, filters, known Sandbox IDs, allocated totals and capacity are
checked. Exact DOM totals, capacity and inventory names must agree with the
captured response. A public read through the other AM must return the same report.
The actual cookie session response must expose the saved visibility; only its
subject, authenticated flag and visibility are retained, without profile fields.

Measured summary cards and inventory readings must follow both modes. Enabled
admin reports require CPU, memory and disk chart responses and visible charts;
personal reports require their default CPU chart. A real null CPU bucket is
located in the accessible chart-data table, paging when necessary. The exact
interval must say Unavailable and expose its actual coverage reason. Generic gap
captions and a chart outline are insufficient. Disabled reports must neither show
measured content nor issue series requests, including responses arriving at close.

Response listeners remain attached through browser close, then join every pending
body before recording final receipts. All captures retain complete serving fields;
parse failures, wrong ingress, extra mutations and unjoined late responses fail.
Only report-creation POSTs are allowed in report browsers; these are real on-demand
computations, never workload actions. Sessions close before the next setting
change. Failure cleanup drains all report sessions before closing the settings
browser and attempting one fresh-CAS restoration, retaining independent failures.
Unknown save/restoration outcomes are never replayed. Parent final database/native
cleanup is still required before campaign acceptance. Offline tests are not a
runtime result or a factory binding.
