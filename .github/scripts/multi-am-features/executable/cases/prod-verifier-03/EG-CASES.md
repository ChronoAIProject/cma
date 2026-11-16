# EG executable cases — author handoff

Part of #2149. Integration owner: developer-15. Author branch:
`tl/cma-13b/coverage-prod-verifier-03`. The frozen assignment remains all 79
rows, EG-001 through EG-079. Currently 37 rows have authored implementations
and 42 remain UNIMPLEMENTED. This handoff is incomplete. No row has a
two-EG/two-AM runtime PASS.

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
Missing fixtures or methods return BLOCKED. No generic adapter-run scenario or
HTTP-success-only fallback substitutes for the real behavior.

Run focused offline verification from repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/helpers.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs
```

Latest focused verification: 22 tests passed, zero failed (author, runner, browser and participant). Tests cover frozen IDs,
exact steps, missing prerequisites, wrong event/delivery joins, duplicated effects,
unsafe-fixture cleanup, cursor cycles, wrong EG replica attribution and shared
browser evidence custody. These tests do not execute the product. The parent
owns N=3 before restart, N=1 after, exact image binding, the sole controlled AM
restart and HPA run. Browser-only and EG-only witnesses retain their true scope.

E1 delayed reply, definition edit and unknown-head release remains unauthored.
It requires the E1 owner's exact local release/fault contract and reviewed head.
E1 execution is lower priority than queued 13b work. Do not operate a release
controller, restart a process or change a NyxID configuration from these cases.

All browser contexts and EG leases register cleanup on acquisition. Preferences
restore before context close. The EG lease must reconcile uncertain work, establish
no-late-effect finality and use exact public cleanup; cleanup failure prevents PASS.
No application process, customer bot, environment or persistent test resource was
created during authoring. No generated cache/download remains. The lead owns
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
| EG-015 | UNIMPLEMENTED | Case authoring remains |
| EG-016 | UNIMPLEMENTED | Case authoring remains |
| EG-017 | UNIMPLEMENTED | Case authoring remains |
| EG-018 | Authored; unverified | browser, eg |
| EG-019 | UNIMPLEMENTED | Case authoring remains |
| EG-020 | UNIMPLEMENTED | Case authoring remains |
| EG-021 | UNIMPLEMENTED | Case authoring remains |
| EG-022 | Authored; unverified | browser, eg |
| EG-023 | UNIMPLEMENTED | Case authoring remains |
| EG-024 | UNIMPLEMENTED | Case authoring remains |
| EG-025 | UNIMPLEMENTED | Case authoring remains |
| EG-026 | UNIMPLEMENTED | Case authoring remains |
| EG-027 | Authored; unverified | browser, eg |
| EG-028 | UNIMPLEMENTED | Case authoring remains |
| EG-029 | UNIMPLEMENTED | Case authoring remains |
| EG-030 | UNIMPLEMENTED | Case authoring remains |
| EG-031 | Authored; unverified | browser, eg |
| EG-032 | Authored; unverified | eg |
| EG-033 | Authored; unverified | eg |
| EG-034 | Authored; unverified | eg |
| EG-035 | Authored; unverified | eg |
| EG-036 | Authored; unverified | eg |
| EG-037 | Authored; unverified | eg |
| EG-038 | Authored; unverified | eg |
| EG-039 | UNIMPLEMENTED | Case authoring remains |
| EG-040 | UNIMPLEMENTED | Case authoring remains |
| EG-041 | UNIMPLEMENTED | Case authoring remains |
| EG-042 | UNIMPLEMENTED | Case authoring remains |
| EG-043 | UNIMPLEMENTED | Case authoring remains |
| EG-044 | UNIMPLEMENTED | Case authoring remains |
| EG-045 | UNIMPLEMENTED | Case authoring remains |
| EG-046 | UNIMPLEMENTED | Case authoring remains |
| EG-047 | UNIMPLEMENTED | Case authoring remains |
| EG-048 | UNIMPLEMENTED | Case authoring remains |
| EG-049 | UNIMPLEMENTED | Case authoring remains |
| EG-050 | UNIMPLEMENTED | Case authoring remains |
| EG-051 | UNIMPLEMENTED | Case authoring remains |
| EG-052 | UNIMPLEMENTED | Case authoring remains |
| EG-053 | UNIMPLEMENTED | Case authoring remains |
| EG-054 | UNIMPLEMENTED | Case authoring remains |
| EG-055 | UNIMPLEMENTED | Case authoring remains |
| EG-056 | Authored; unverified | eg |
| EG-057 | UNIMPLEMENTED | Case authoring remains |
| EG-058 | UNIMPLEMENTED | Case authoring remains |
| EG-059 | UNIMPLEMENTED | Case authoring remains |
| EG-060 | Authored; unverified | eg |
| EG-061 | Authored; unverified | eg |
| EG-062 | Authored; unverified | eg |
| EG-063 | UNIMPLEMENTED | Case authoring remains |
| EG-064 | UNIMPLEMENTED | Case authoring remains |
| EG-065 | Authored; unverified | eg |
| EG-066 | UNIMPLEMENTED | Case authoring remains |
| EG-067 | UNIMPLEMENTED | Case authoring remains |
| EG-068 | UNIMPLEMENTED | Case authoring remains |
| EG-069 | Authored; unverified | eg |
| EG-070 | UNIMPLEMENTED | Case authoring remains |
| EG-071 | UNIMPLEMENTED | Case authoring remains |
| EG-072 | UNIMPLEMENTED | Case authoring remains |
| EG-073 | Authored; unverified | eg |
| EG-074 | Authored; unverified | eg |
| EG-075 | Authored; unverified | eg |
| EG-076 | Authored; unverified | eg |
| EG-077 | Authored; unverified | eg, eg_accounts |
| EG-078 | Authored; unverified | eg, eg_accounts |
| EG-079 | UNIMPLEMENTED | Case authoring remains |
