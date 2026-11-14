# Combined Batch9/Batch10 / U1–U3 / S1–S3 timeline — 14 November amendments

All times are Asia/Singapore (SGT), with dates explicit across midnight.
The lead 20:30 revision supersedes the earlier morning schedule. Engineer01
records engineer02's reported lead 20:45 amendment for component checkpoints and
Batch10 before Activity; its approval/actuals remain in the execution owner packet.
Deployment-engineer02 owns Batch9/Batch10, Activity and U1–U3; deployment-engineer01
owns S1–S3. **Every stage needs its own fresh lead approval and observed prior
success criteria.** Planned times supply neither a grant nor actual start/end.
If a stage fails, perform approved rollback, report and let the lead re-slot it.
The final deadline remains **15 November 18:00**.

| Planned time / condition | Owner | Operation and required ordering |
|---|---|---|
| 14 Nov 21:00 | engineer02 | Batch9 primary rollout at N=1 with shipped defaults: multi-replica false, rollups OFF, web recovery OFF. Engineer02 reports lead approval for the 21:00 target; actual rollout/cohort results remain pending until observed. Roll API → EG → web sequentially, with a separate 15-minute checkpoint and auto-rollback for EACH component. Retain all three actual results. |
| After ALL Batch9 component checkpoints PASS | engineer02 | Queue Batch10 API-only build/push at source `47809dbcf2193b4e28fca679f1b84bd0c27ac82c`. This build ordering supplies no deployment grant. |
| 14 Nov 21:30–22:30 | log01 | S1 60-minute baseline on the actual Batch9 image at N=1. Bind actual source/image/state and coverage; retain the standing 24h context. |
| 14 Nov approximately 22:30 | engineer01 | S1 AM=2 plus PDB only after ALL Batch9 component checkpoints and S1 dry-run projections PASS using merged #2043 controller develop `9be345523fd2768d8e8daf1c0f4ccd487191f9b0` or a descendant retaining its correction. Observe real production traffic under the existing low-N rule. |
| 15 Nov approximately 00:30 | engineer01 | S2 EG=2 only after S1 success. Complete cutover and the full soak before 02:00; reserve recovery time and retain the controller's 01:58 write margin. If the remaining time cannot satisfy the criteria, report for lead re-slotting. |
| 15 Nov 02:00–02:59 | all operators | No writes, including recovery. The blackout is unchanged. |
| 15 Nov 03:00 target | engineer02 | Separately approved Batch10 API-only rolling rollout at AM=2/PDB, with EG/web retained on Batch9. Bind selected image and actual start/end/results; they remain unset until supplied. |
| After observed Batch10 API success | engineer02 | Activity index → U1 collection/index Job → U2, each after predecessor success and a fresh grant. Activity's exact start is pending Batch10 success, not fixed 03:00. U1 credential grant remains `GRANT_NOOP`. |
| U1 succeeds and U2 can start strictly before 15 Nov 04:30 | engineer02 | Start U2 under its own fresh grant. Exactly 04:30 or later uses the fallback after actual S3 success. |
| 15 Nov approximately 05:30–06:00 | engineer01 | S3 CPU quota 64/128 → HPA → sequential S3b exercise from merged #2038. Bind the final selected controller, observed prior success and separate grants; complete all required observation/drain/soak without compressing them. |
| After actual S3 success, if early U2 was missed | engineer02 | Start U2 under a fresh grant. Nominal S3 start/end is not completion evidence. |
| After U2 `pass_complete` | engineer02 | U3 starts only at least 15 minutes away from every actual/planned S-stage start, confirmed with engineer01 and recomputed whenever the lead re-slots a stage. |
| 15 Nov 09:00–16:00 | prod-verifier01 | M8 production verification using the existing kit, after observed prerequisite success and the fresh stage grant. |
| 15 Nov 18:00 | all reporting owners / lead | Final report due. Stage reports remain due by actual soak end E+15. |

U3 requires an absolute separation of at least 15 minutes from each S-stage
start. Confirm the chosen S3 start within its 05:30–06:00 planning range before
U3 scheduling; do not treat an unresolved range as a measured timestamp.
Every actual timestamp and result stays pending until observed. Re-slotting a
stage also rebinds its baseline, read windows and U3 separation; it never erases
previous failures or grants automatic advancement.

## Baseline packet times

Each log before interval is `[actual_operation_start - 60 minutes, actual_operation_start)`
on the actual serving image. The following are planned packet bounds; actual observations remain pending:

| Stage | Planned before interval, SGT |
|---|---|
| S1 | 14 Nov 21:30–22:30, on the Batch9 image |
| S2 | 14 Nov 23:30–15 Nov 00:30 |
| S3 at 05:30 | 15 Nov 04:30–05:30 |
| S3 at 06:00 | 15 Nov 05:00–06:00 |

For S3, engineer02 supplies observed post-Batch10 API and retained Batch9 EG
source/image/replica lifetimes. Keep rollout boundaries explicit and compare
matching serving cohorts; do not pool pre/post-rollout measurements or populate
actuals from the 03:00 target. A missing matching interval remains explicit.

Log01 rebinds the interval when the actual operation start moves. Existing
source/image/state matching, retained failures and coverage uncertainty remain.
These 60-minute log windows do not replace or extend prod-verifier02's 1800-second
READ session. The read owner supplies its actual scheduled bounds separately.

## Match S1 observations to backfill state

Engineer02 publishes actual U1 success, U2 start, current state and
`pass_complete` timestamps/identities through the existing safe handoff.
Engineer01 records them with actual S1 operation/cutover/soak times and gives
log-engineer01 the same packet. Scheduled or future timestamps remain unset
as actuals. The source/image/replica join must describe the serving processes.

Log01 tags every S1 latency/error comparison with `backfill_active` as true,
false or unknown, plus its observed interval/source. The AM1 baseline must be
from the **same backfill state** as the compared S1 cohort. Retain the standing
24h baseline and use the 21:30–22:30 Batch9 60-minute before-window for the
planned 22:30 S1 start. If the lead re-slots S1, refresh/rebind that before-window
to the actual operation start. A quiet pre-backfill baseline cannot qualify an
active-backfill comparison. Missing state or matched baseline remains explicit.

If backfill starts or completes within a window, split latency/error results at
the observed state transition, keeping original per-replica/route counts,
p50/p95 and errors for each state. Do not pool unlike states to hide a regression
or attribute a maintenance effect to replica count. Keep initial30-minute and
full extended accepted-command N on the same original S, deduplicated once;
state segmentation never resets the stage's cumulative failures or denominator.

S1's existing real-production-traffic/read-only policy, N<20 same-observation
extension and disclosed low-N suite-plus-zero-error acceptance remain unchanged.
Prod-verifier02 rebinds the owned READ window to the revised stage packet;
the existing NyxID slot and pacing remain. The 1800-second synthetic read session
never extends/restarts automatically and is distinct from the 60-minute log baseline.
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
