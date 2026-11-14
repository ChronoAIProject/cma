# Combined U1–U3 / S1–S3 timeline — lead ruling 14 November 19:56

All times are SGT on15 November. Deployment-engineer02 owns U1–U3 and the
M4/index sequence; deployment-engineer01 owns S1–S3. **Every U and S stage needs
its own fresh lead approval at execution time.** This schedule supplies neither
a grant nor an actual start/end. The 02:00–02:59 blackout remains in force.

| Time / condition | Owner | Operation and required ordering |
|---|---|---|
| 03:00 | engineer02 | Batch9, then its image soak, then the Activity index. |
| After successful Activity index | engineer02 | U1 collection-and-index Job only. Credential grant is `GRANT_NOOP`: the AM credential already has `readWrite` + `dbAdmin`; no new credential/role action. |
| U1 succeeds and U2 can start **before04:30** | engineer02 | Start U2 immediately under its own fresh grant. Exactly04:30 or later uses the fallback. |
| 06:00 | engineer01 | S1 starts on schedule using the merged fixed controller, including when U2 remains active. Replica maintenance is lease-fenced; active backfill alone does not delay S1. |
| 06:45 fallback, after S1 soak | engineer02 | If the early U2 start was missed, start U2 after S1's actual soak ends, targeting06:45. Preserve the existing S1 low-N extension: if it reaches past06:45, wait for that actual end and the fresh U2 grant. Do not launch during the protected soak to satisfy a nominal time. |
| After U2 `pass_complete`, outside S-stage exclusions | engineer02 | U3 may start only after actual successful U2 completion and outside the ±15-minute start exclusions below. |
| 08:30 | engineer01 | S2, subject to its own prerequisites/PASS and fresh grant. |
| 10:30 | engineer01 | S3, subject to its own prerequisites/PASS and fresh grant; includes the separately granted sequential S3b exercise. |
| 13:00–17:00; report by18:00 | assigned verification owners | Final production verification and owner report. Each soak report is also due at its actual E+15. |

U3 **launch** exclusions are05:45–06:15 around S1,08:15–08:45 around S2,
and10:15–10:45 around S3. Treat the boundaries conservatively as excluded.
Recompute these intervals if the lead changes any S-stage start; engineer02
must confirm the current S schedule with engineer01 before launching U3.
This rule does not claim that U3 has completed or that U2 is `pass_complete`.

## Match S1 observations to backfill state

Engineer02 publishes actual U1 success, U2 start, current state and
`pass_complete` timestamps/identities through the existing safe handoff.
Engineer01 records them with actual S1 operation/cutover/soak times and gives
log-engineer01 the same packet. Scheduled or future timestamps remain unset
as actuals. The source/image/replica join must describe the serving processes.

Log01 tags every S1 latency/error comparison with `backfill_active` as true,
false or unknown, plus its observed interval/source. The AM1 baseline must be
from the **same backfill state** as the compared S1 cohort. Retain the standing
24h baseline and refresh the existing60-minute before-window immediately before
S1's actual operation start. A quiet pre-backfill baseline cannot qualify an
active-backfill comparison. Missing state or matched baseline remains explicit.

If backfill starts or completes within a window, split latency/error results at
the observed state transition, keeping original per-replica/route counts,
p50/p95 and errors for each state. Do not pool unlike states to hide a regression
or attribute a maintenance effect to replica count. Keep initial30-minute and
full extended accepted-command N on the same original S, deduplicated once;
state segmentation never resets the stage's cumulative failures or denominator.

S1's existing real-production-traffic/read-only policy, N<20 same-observation
extension and disclosed low-N suite-plus-zero-error acceptance remain unchanged.
The owned1800-second synthetic read session never extends/restarts automatically.
Any actual safety/accepted-work/absolute failure still triggers the immediate
log01→engineer01 incident/ACK route and the approved compatible recovery.

## Direct coordination

Engineer01 and engineer02 use guarded `team.sh peer-send ... --force` with safe
files; reread the current team policy before sending. Include selected source,
actual status/timestamps, pending fields and the next approval owner. Delivery
exit0 is not an operation grant, execution ACK or completion proof. Log01 uses
the [published incident route](stage-command-review.md) for failures; do not
wait for the final E+15 report. See the [common runbook](deployment-runbooks.md)
and [S1 observation sequence](s1-execution.md) for execution details.
