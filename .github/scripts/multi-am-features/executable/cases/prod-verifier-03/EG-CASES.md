# EG executable cases — author handoff

Part of #2149. Integration owner: developer-15. Author branch:
`tl/cma-13b/coverage-prod-verifier-03`. The frozen assignment remains all 79
rows, EG-001 through EG-079. All 79 rows now have authored implementations.
No row has a two-EG/two-AM runtime PASS; runtime execution remains parent-owned.
Registration and matching step IDs do not prove behavioral coverage of every
checklist clause. The authored cases still need verification with authentic
parent fixtures and the compiled product.

`index.mjs` exports individual definitions with the exact frozen step IDs.
Browser modules perform real Playwright UI actions, including personal report
completion and contents, permission/session boundaries, revisioned channel edits
and deletion. Platform cases require verified synthetic ingress through real EG
processes, independently observed non-owner AM attribution, provider acceptance
and actual synthetic platform delivery. Local fixtures do not establish live
third-party or production bot acceptance. Tool cases assert exact identities,
reply effects, pagination, current permissions and no-effect read behavior.

The shared browser adapter now exists at `executable/browser.mjs`. Cases use its
`open/evidence` interface and local session origin; browser attribution is retained
on success and failure. The parent must install it with its leased Chromium and
compiled web artifact. Specialized `eg_browser`, `eg` and `eg_accounts` fixtures
remain prerequisites. Their concrete methods and setup are in `fixtures.json`.
Authentic setup exports include `setups.channel_edit` plus parent-bound setup
functions for every remaining authored scenario. `channelEditSetup` creates and
cleans up a reserved disposable channel through real EG HTTP; `createEgTransport`
checks actual replica witnesses and exact source/image binding. The remaining
setups bind the parent's real disposable EG lease and seed configuration only;
they do not execute a case or return a result oracle. They need the explicit
`primitives.eg` contract in `fixtures.json`. Missing fixtures or methods return BLOCKED. No generic adapter-run scenario or
HTTP-success-only fallback substitutes for the real behavior.

Run focused offline verification from repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/helpers.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs
```

Focused verification (40 tests in the current source):

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs .github/scripts/multi-am-features/executable/serving.test.mjs .github/scripts/multi-am-features/executable/eg-evidence.test.mjs
```

Latest full executable verification: 269 tests passed, zero failed. The focused suite covers frozen IDs,
exact steps, missing prerequisites, wrong event/delivery joins, duplicated effects,
unsafe-fixture cleanup, cursor cycles, wrong EG replica attribution and shared
browser evidence custody. These tests do not execute the product. The parent
owns N=3 before restart, N=1 after, exact image binding, the sole controlled AM
restart and HPA run. Browser-only and EG-only witnesses retain their true scope. AM-backed EG turns now require actual `amRequests` with joined owner proofs; `retainTurn` publishes `multi-am-eg-evidence-v1` for shared-runner ingestion. Requested AM labels never satisfy that evidence.

The 20 latest cases declare `user-fixtures` and use the real registry dispatcher;
an unavailable parent factory blocks before feature execution. EG-023 requests
`producer_edit`, separate from EG-022's one-source `channel_edit` setup. Cleanup
aliases reuse the same promise, retain the lease receiver and preserve rejection.
The HTTP helper forwards expected statuses and supports empty 204/404 bodies.
Regression tests exercise the dispatcher and local HTTP transport, not a product
campaign.

EG-048 now dispatches all four required steps. It fills the actual Consumer
wizard, verifies its saved intent before submitting, and forwards exactly one
real AM create request. The test then loses that accepted response at the browser
boundary, verifies the UI retains the same body/key, and polls the observed Bot
ID through the opposite **actual** serving AM. It checks exact profile revisions,
configured services/image/workspace, source admission and reply sinks, then opens
the linked EG channel in the UI. The browser channel response must join an actual
EG receipt; it cannot inherit AM attribution. Parent setup now requires the
`wizard` inputs, `retainCreationIntent`, `creationRecords`, and an `eg_browser`
proxy binding with `egEvidence`. Missing setup stays BLOCKED. Setup supplies a
separate existing channel so the newly created channel is selected explicitly.
Eight new offline fault/receipt regressions plus an independent all79 step-dispatch
check cover these changes; they do not establish compiled-UI or runtime PASS.

E1 delayed reply, definition edit and unknown-head release is outside the 79
frozen EG rows but remains outstanding in the original task scope. Its supplemental
journey is unauthored; it requires the E1 owner's exact local release/fault contract
and reviewed head, with execution parent-owned. Do not operate a release controller,
restart a process or change a NyxID configuration from these cases.

All browser contexts and EG leases register cleanup on acquisition. Preferences
restore before context close. The EG lease must reconcile uncertain work, establish
no-late-effect finality and use exact public cleanup; cleanup failure prevents PASS.
No application process, customer bot, environment or persistent test resource was
created during authoring. The temporary npm cache was removed. The pinned ignored `multi-am-features/node_modules` dependency remains available for shared WebSocket tests. The lead owns
worktree cleanup after integration. Developer15 owns the consolidated PR and
develop merge; no competing PR or default-branch merge was created.

| Case | Implementation | Runtime prerequisite |
| --- | --- | --- |
| EG-001 | Authored; unverified | browser |
| EG-002 | Authored; unverified | browser, eg_browser |
| EG-003 | Authored; unverified | browser, eg_browser |
| EG-004 | Authored; unverified | browser |
| EG-005 | Authored; unverified | browser |
| EG-006 | Authored; unverified | browser |
| EG-007 | Authored; unverified | browser, eg_browser |
| EG-008 | Authored; unverified | browser, eg_browser |
| EG-009 | Authored; unverified | browser, eg_browser |
| EG-010 | Authored; unverified | browser, eg_browser |
| EG-011 | Authored; unverified | browser, eg_browser |
| EG-012 | Authored; unverified | browser, eg_browser |
| EG-013 | Authored; unverified | browser, eg_browser |
| EG-014 | Authored; unverified | browser, eg_browser |
| EG-015 | Authored; unverified | browser, eg_browser |
| EG-016 | Authored; unverified | browser, eg_browser |
| EG-017 | Authored; unverified | browser, eg_browser |
| EG-018 | Authored; unverified | browser, eg |
| EG-019 | Authored; unverified | authentic parent fixture setup required |
| EG-020 | Authored; unverified | authentic parent fixture setup required |
| EG-021 | Authored; unverified | authentic parent fixture setup required |
| EG-022 | Authored; unverified | browser, user-fixtures; implemented channel_edit setup |
| EG-023 | Authored; unverified | authentic parent fixture setup required |
| EG-024 | Authored; unverified | authentic parent fixture setup required |
| EG-025 | Authored; unverified | authentic parent fixture setup required |
| EG-026 | Authored; unverified | authentic parent fixture setup required |
| EG-027 | Authored; unverified | browser, eg |
| EG-028 | Authored; unverified | authentic parent fixture setup required |
| EG-029 | Authored; unverified | authentic parent fixture setup required |
| EG-030 | Authored; unverified | authentic parent fixture setup required |
| EG-031 | Authored; unverified | browser, eg |
| EG-032 | Authored; unverified | eg |
| EG-033 | Authored; unverified | eg |
| EG-034 | Authored; unverified | eg |
| EG-035 | Authored; unverified | eg |
| EG-036 | Authored; unverified | eg |
| EG-037 | Authored; unverified | eg |
| EG-038 | Authored; unverified | eg |
| EG-039 | Authored; unverified | eg |
| EG-040 | Authored; unverified | eg |
| EG-041 | Authored; unverified | eg |
| EG-042 | Authored; unverified | eg |
| EG-043 | Authored; unverified | eg |
| EG-044 | Authored; unverified | authentic parent fixture setup required |
| EG-045 | Authored; unverified | authentic parent fixture setup required |
| EG-046 | Authored; unverified | authentic parent fixture setup required |
| EG-047 | Authored; unverified | authentic parent fixture setup required |
| EG-048 | Authored; unverified | authentic parent fixture setup required |
| EG-049 | Authored; unverified | authentic parent fixture setup required |
| EG-050 | Authored; unverified | browser, eg |
| EG-051 | Authored; unverified | browser, eg |
| EG-052 | Authored; unverified | browser, eg |
| EG-053 | Authored; unverified | browser, eg |
| EG-054 | Authored; unverified | browser, eg |
| EG-055 | Authored; unverified | authentic parent fixture setup required |
| EG-056 | Authored; unverified | eg |
| EG-057 | Authored; unverified | eg |
| EG-058 | Authored; unverified | eg |
| EG-059 | Authored; unverified | eg |
| EG-060 | Authored; unverified | eg |
| EG-061 | Authored; unverified | eg |
| EG-062 | Authored; unverified | eg |
| EG-063 | Authored; unverified | eg |
| EG-064 | Authored; unverified | eg |
| EG-065 | Authored; unverified | eg |
| EG-066 | Authored; unverified | eg |
| EG-067 | Authored; unverified | eg |
| EG-068 | Authored; unverified | eg |
| EG-069 | Authored; unverified | eg |
| EG-070 | Authored; unverified | authentic parent fixture setup required |
| EG-071 | Authored; unverified | authentic parent fixture setup required |
| EG-072 | Authored; unverified | authentic parent fixture setup required |
| EG-073 | Authored; unverified | eg |
| EG-074 | Authored; unverified | eg |
| EG-075 | Authored; unverified | eg |
| EG-076 | Authored; unverified | eg |
| EG-077 | Authored; unverified | eg, eg_accounts |
| EG-078 | Authored; unverified | eg, eg_accounts |
| EG-079 | Authored; unverified | eg |
