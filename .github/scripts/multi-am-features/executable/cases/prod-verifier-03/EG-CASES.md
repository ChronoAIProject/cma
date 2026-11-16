# EG executable cases — author handoff

Part of #2149. Integration owner: developer-15. Author branch:
`tl/cma-13b/coverage-prod-verifier-03`. The frozen assignment remains all 79
rows, EG-001 through EG-079. This first slice is incomplete and is not runtime
acceptance. No row has a two-EG/two-AM runtime PASS.

`index.mjs` exports individual case definitions with the exact frozen step IDs.
`browser.mjs` performs compiled UI actions for EG-001, 003, 005–008.
`platform.mjs` exercises verified synthetic ingress, AM acceptance, observed
platform delivery, contextual follow-up, duplicate/denied events, and durable
response recovery for EG-032–038 and EG-056. Platform cases require the real EG
processes with a local synthetic platform/provider. They do not establish live
third-party or production bot acceptance.

Every remaining assigned row is still UNIMPLEMENTED, not a passing declaration.
The E1 delayed reply, definition edit and unknown-head release extension also
remains to be authored and coordinated with its owner. Its execution is lower
priority than queued 13b runs.

Fixture requirements are in `fixtures.json`. Missing `browser` or `eg`, or a
missing adapter method, returns BLOCKED. The shared environment owner must
implement the named adapters; the author cases do not launch applications.
`eg_accounts` records the additional identity variants needed for the remaining
organization, service-only and authorization rows. Credential values remain
inside the adapter and are never included in case observations.

Run focused offline verification from repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/helpers.test.mjs
```

These checks validate exact registration, missing-fixture failure and negative
event/delivery evidence. They are author tests, not application verification.
The parent owns N=3 before restart, N=1 after, exact image binding, the sole
controlled AM restart and HPA run. EG-only and browser-only witnesses retain
their scope. The parent must resolve its `steady` versus `before_restart` phase
mapping for read-only rows before final coverage accounting.

Browser contexts and EG fixture leases register cleanup immediately upon
acquisition. Preferences restore before context close. The EG lease must
reconcile uncertain work, establish no-late-effect finality, and use public
deletion of exact test-owned resources; cleanup failure prevents PASS.
No environment, application process, customer bot or persistent test resource
was created by this authoring work. The lead owns worktree cleanup after
integration. No separate PR or develop merge is opened by this contributor.
