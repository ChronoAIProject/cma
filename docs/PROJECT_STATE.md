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
| [Project 9](https://github.com/orgs/ChronoAIProject/projects/9) | Exact Week and assignee define the owner's sprint scope; include open and closed issues and PRs across repositories |
| [Dispatch and decision ledger #1493](https://github.com/ChronoAIProject/cma/issues/1493#issuecomment-6773214979) | Friday triage, responsibility routes, merge windows, unresolved dependencies and capacity confirmation |
| [W45 report #1400](https://github.com/ChronoAIProject/cma/issues/1400#issuecomment-6773124853) | Completed November 2–8 historical report; ongoing engineering work stays in its linked issues |
| [Weekend retrospective #1558](https://github.com/ChronoAIProject/cma/issues/1558#issuecomment-6773126440) | Documented November 7–8 misses and remaining exit evidence; later fixes are not backdated |
| [QA policy #1572](https://github.com/ChronoAIProject/cma/issues/1572#issuecomment-6773213790) | Draft execution rules and pending product/QA confirmations; not permission to run production mutations |

The Friday engineering cut line is **2026-11-13 15:00 SGT** for the assigned
Project 9 items with **Week = 11-09~11-15**. Other items follow their own project
week; related gates do not automatically join this sprint. November 14 is a
Saturday. Use the ledger for the active train and review schedule. Capacity,
backup coverage and reserves require product-owner confirmation; role routing and
automation duration are not evidence of available staffing. An item's own later
steps remain due on their recorded dates.

## Current evidence boundaries

- **Environment:** the November 10 owner decision records production as the sole
  pre-launch staging environment. See the [decision record on #1581](https://github.com/ChronoAIProject/cma/issues/1581#issuecomment-6770063716).
  This preserves GET-only smoke and all target, credential and fixture isolation
  boundaries. Production changes belong to the designated release owner.
- **Nightly:** [#1592](https://github.com/ChronoAIProject/cma/pull/1592) fixed the
  supported-Node browser install; [#1626](https://github.com/ChronoAIProject/cma/pull/1626)
  masked the smoke identifier. [Scheduled run 42884394431](https://github.com/ChronoAIProject/cma/actions/runs/42884394431)
  passed three GET-only cases with artifacts for the November 11 slot; browser
  installation took 24 seconds. [Manual run 42887023846](https://github.com/ChronoAIProject/cma/actions/runs/42887023846)
  also passed, with a 21-second browser install, but adds no scheduled night.
  [#1581](https://github.com/ChronoAIProject/cma/issues/1581) records the completed
  narrow fix. Never copy the public Trigger ID or credentials into this index.
- **Qualification:** [#1471](https://github.com/ChronoAIProject/cma/issues/1471)
  owns continuous evidence and [#1556](https://github.com/ChronoAIProject/cma/issues/1556)
  owns the scheduled qualification window. Manual dispatch cannot replace a
  scheduled success. The evidence above counts one scheduled night, not the
  complete window; follow the gate issue for current counts and the product
  owner's cutoff decision. A Friday engineering delivery cannot complete a
  future 20-night, recovery, soak or final service-observation window.
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
