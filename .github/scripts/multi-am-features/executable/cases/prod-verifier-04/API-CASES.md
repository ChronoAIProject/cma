# API supplement author handoff

Issue #2149. The frozen assignment contains 87 API operations, a denominator
separate from C275. The 14 former manual admin rows moved to Developer01 with
the complete 76-row admin assignment. Developer15 owns integration and the
sole application environment; this contribution does not launch an app.

The first slice exports 38 real HTTP cases for Agent/Workspace/Sandbox profiles,
marketplace, Event Worker Profiles and authoring catalogs. Each preserves the
four required catalog steps. The family journey creates each profile kind,
publishes revisions, changes actual content, rejects stale draft versions,
checks immutable history, exercises public/private visibility and forks,
checks alternate-AM idempotency, isolates an outsider and retires resources.
The exact assigned method/path then executes separately with concrete content
and identity assertions. Event Worker Profiles use expected_revision; mutable
profile drafts use draft_version. These are global stores, so there is no
invented Agent owner evidence.

The named fixture requirements are in fixtures.json. Only authoring repository
and services cases currently need api-fixtures; all cases require the synthetic
transport actors and local policies in its environment section. Remaining API
families are under implementation and are not registered as passing stubs.

Run focused author checks from the repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-04/*.test.mjs
```

The first slice has seven passing offline verifier tests, including the complete
38-case contract traversal and fault injection for incorrect content, owner
leakage, mutable published revisions, unknown create custody and cleanup failure.
These checks use a transport model and do not establish CMA feature PASS.
The shared runner must execute the integrated cases N=3 before and N=1 after
its controlled owner restart, against the newest exact image with both AMs.

Cleanup is registered before each creation and uses the original resource ID.
If a mutation fails after acceptance with an attached response, its ID remains
in cleanup custody. If creation custody is unknown, cleanup fails explicitly
for parent namespace reconciliation. Cleanup failures never replace the primary
failure. No uncertain mutation is automatically retried with a new key.

Runtime evidence remains missing until Developer15 installs the named fixtures
and runs the integrated suite. Public reporting contains only counts and hashes;
synthetic identities and request bodies belong to private parent evidence.
