# Stage checklist references and verification

Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016). Owner: solution-architect-03. **Frozen 14 November 2026, 16:17 SGT**, before 15 November 00:00. Deliverables: [three stage pages](stage-go-checklists.md) and [command/rollback details](stage-command-review.md). The lead retains GO and stage advancement; deployment-engineer-01 executes.

## Controlling references

- Lead plan: `/Users/shining/code/work/tech-lead/notes/2026-11-14-hard-deadline-plan.md`. S1 06:00, S2 08:30, S3 10:30, final 18:00 SGT; no writes 02:00–02:59. Local gates and admin integration; no CI wait.
- Architect-04: `/Users/shining/code/work/tech-lead.staff/solution-architect-04/`, files `production-stages.md`, `essentials.md`, `contract-addendum-a2.md`, `contract-addendum-a3.md` and `contract-addendum-work-recovery-20261114.md`. Current K01–K25 and A3 supersede old full-matrix repetitions, schedules and blanket noise rules.
- Manifest baseline #1968: `d38966aab5c77fa2262da2d14ffba0da84cd958c`; corrected EG native sleep #1996: `37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5`. Inspected integration: `15306eed8aacb67964f95442c1f29ba47d161502`. This identifies source, not final production images or runtime qualification.
- Engineer packet root: `/Users/shining/code/work/.tl-worktrees/cma-cmaeg-release/.github/.cmaeg-local/cma-scale-20261113/`. Current runbook/templates are `bindings/corrected-eg-37f2bedbf-deadline-r1`; historical helper interface is `frozen/a2-r5`. The corrected packet has no helper scripts and cannot be an executable root. Historical EG exec derivatives remain withdrawn.
- Developer-16 input: `/Users/shining/code/work/tech-lead.staff/developer-16/deadline-20261115/stage-checklist-handoff-r1/HANDOFF.md`. Correction source and operation identities were checked against this handoff.
- Engineer's later planning memo: `supplements/measurement-decision-20261114-r1/decision-memo.md` under the engineer packet root. Longer S1/S2 observations are proposed, not approved: stabilization by 06:02/08:32, observation ends 07:50/10:20, reports 08:05/10:25. Current passive comparison requires three baseline and three post windows. These times alone prove neither independence nor adequate samples. Retain the after03:00 index order unless the lead changes it.

## Review and checks

Deployment-engineer-01 read the complete three-file draft. Its review is retained at `reviews/architect03-stage-checklist-r1/review.md` under the engineer packet root. Incorporated all concrete corrections: actual distinct-host placement is checked after start-two; the metadata packet cannot be an executable root; one post-soak window is insufficient for the current confidence method; observing five is necessary only for a claim that five was exercised. K14 still covers configured maximum capacity; K25 still requires useful automatic up/down. Developer-16 supplied the exact manifest/operation and capacity handoff.

Completed local checks on 14 November:

- Manifest contract: both roles PASS. Existing manifest suite: **21 tests, 120 assertions, 0 failures/errors/skips**.
- Corrected operation comparison: **15 of 16 unchanged**; sole delta is EG exec → native sleep30. Both prepare projections preserve fields/env and do not write replicas. All 14 copied manifest/build inputs match the correction commit.
- Complete new runbook and execution metadata read. The supported controller route, failed-start recovery, final images, K13 early drain, qualified handoff/lifetime and full capacity remain missing. No additional documentary approval closes these product/execution gaps.
- Final document checks PASS: three stage pages, four quoted Bash blocks syntax-valid, eight local links resolve, all K01–K25 mapped, every file below500 lines, and no whitespace errors. Shell checks are syntax only; no production commands were executed.

The lead's speed directive applies: evidence is source, tests, commit and concise results. No further intermediate sealing, checksum handoffs or unchanged-byte review rounds are required. No application build, container, production query, server dry-run, helper execution or activation was performed for this task.

## Essential ownership and stage coverage

| Rows | Required evidence handoff |
| --- | --- |
| K01 | Integrator/local verifiers: final source, local gates, build and configuration before M4/S1. |
| K02 | Engineer/production verifiers: accepted-work conservation at every stage; no empty denominator. |
| K03–K08 | AM owners/local verifier: local races/kill; S1 observations, preserved through S2/S3. |
| K09–K11 | EG owners/local verifier: local admission/provider/loss; authorized S2/S3 observations. |
| K12 | Local verifier/developer-17: actual M3 2→4→2; S3 automatic behavior is separate. |
| K13–K15 | Developer-16, drain/capacity owners and engineer: drain, placement, capacity, compatible receivers/floors before activation and throughout transitions. |
| K16–K21 | Analyst/client/production verifiers: complete journeys, distinct routes, A3 classes/confidence; M8 evidence by17:00. |
| K22 | Engineer: approved S1/S2 operations, no overlap and actual <120s service-restoration clock. |
| K23 | Engineer/provider monitor: fresh S1/S2 stable soaks, closed canaries and named24-hour monitor. |
| K24 | Engineer/lead: state-specific compatible recovery before each stage; M8 fixes forward only. |
| K25 | Engineer/developer-17/production verifiers: useful automatic up/down, final stable-two soak and M8/final reporting. |

Reviewer owns the checklist decision record; these evidence handoffs confer no production permissions. Publish through developer-13's named integration branch and milestone PR, with `Closes #2016`; no independent develop PR. Task scratch is removed before final handoff, and the lead retains worktree cleanup.
