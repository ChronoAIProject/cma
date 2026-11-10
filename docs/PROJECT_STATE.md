# CMA Project State

Index refreshed: **2026-11-11 (Asia/Singapore)**.

GitHub issues and PR evidence are the source of task state. This index identifies
where to find scope, dispatch decisions and acceptance; it is not a release
sign-off or a copy of the backlog. Follow [AI_WORKFLOW.md](AI_WORKFLOW.md) when
claiming, refreshing or closing work.

## Coordination records

| Record | Authority and evidence boundary |
|---|---|
| [Release overview #1573](https://github.com/ChronoAIProject/cma/issues/1573) | Public scope, owner decisions, hard gates and qualification dates; implementation alone does not satisfy them |
| [Sprint #1559](https://github.com/ChronoAIProject/cma/issues/1559) | Current sprint outcomes and exit conditions |
| [Dispatch and decision ledger #1493](https://github.com/ChronoAIProject/cma/issues/1493#issuecomment-6773214979) | Friday triage, responsibility routes, merge windows, unresolved dependencies and capacity confirmation |
| [W45 report #1400](https://github.com/ChronoAIProject/cma/issues/1400#issuecomment-6773124853) | Completed November 2–8 historical report; ongoing engineering work stays in its linked issues |
| [Weekend retrospective #1558](https://github.com/ChronoAIProject/cma/issues/1558#issuecomment-6773126440) | Documented November 7–8 misses and remaining exit evidence; later fixes are not backdated |
| [QA policy #1572](https://github.com/ChronoAIProject/cma/issues/1572#issuecomment-6773213790) | Draft execution rules and pending product/QA confirmations; not permission to run production mutations |

The Friday engineering cut line is **2026-11-13 15:00 SGT**; November 14 is a
Saturday. Use the ledger for the active train and review schedule. Named capacity,
backup coverage and reserves require product-owner confirmation; role routing and
automation duration are not evidence of available staffing.

## Current evidence boundaries

- **Environment:** the November 10 owner decision records production as the sole
  pre-launch staging environment. See the [decision record on #1581](https://github.com/ChronoAIProject/cma/issues/1581#issuecomment-6770063716).
  This preserves GET-only smoke and all target, credential and fixture isolation
  boundaries. Production changes belong to the designated release owner.
- **Nightly:** [#1592](https://github.com/ChronoAIProject/cma/pull/1592) merged the
  supported-Node browser-install fix. [Branch run 42833983221](https://github.com/ChronoAIProject/cma/actions/runs/42833983221)
  completed Chromium installation in 24 seconds, then failed a missing-variable
  prerequisite. This is install evidence, not remote smoke success. Follow
  [#1581](https://github.com/ChronoAIProject/cma/issues/1581) for the published
  Trigger prerequisite and the post-merge proof; never copy its public ID or
  credentials into this index.
- **Qualification:** [#1471](https://github.com/ChronoAIProject/cma/issues/1471)
  owns continuous evidence and [#1556](https://github.com/ChronoAIProject/cma/issues/1556)
  owns the scheduled qualification window. Manual dispatch cannot replace a
  scheduled success. A Friday engineering delivery cannot complete a future
  20-night, recovery, soak or final service-observation window.
- **Production:** source, a closed implementation issue or green CI does not
  prove the current image, configuration, provider behavior or retained-user
  continuity. Consult the release evidence for the exact candidate and request
  missing observations through the designated release owner.

## Repository contracts and operational references

- [CLAUDE.md](../CLAUDE.md): ownership, structure, compatibility and CI rules.
- [Deployment readiness](CMA_Deployment_Readiness.md): capability and evidence
  boundaries; no unverified public promises.
- [Release runbook](Release.md) and [rolling deployment](CMA_Rolling_Deployment.md):
  approved promotion, retained assets and continuity constraints.
- [Native production verification](CMA_Native_Production_Verification.md):
  candidate-specific verification; an index does not replace its required proof.

Refresh an entry when its authoritative record moves or its evidence boundary
changes. Do not resurrect the November 2 snapshot's ready-task list, infer another
worker's status from a local checkout, or mark a capability verified without a
current linked result.
