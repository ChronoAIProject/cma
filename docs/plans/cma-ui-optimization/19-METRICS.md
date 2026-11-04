# Ticket 19 measured denominators

Reference: immutable `ticket19-proof/attempt-38`. Collector timestamps all use
the same host clock; public transcript and native dispatches establish business
outcomes. Page-memory suggestion events are not used as cross-reload denominators.
This is one controlled owner/Agent/Sandbox, not production analytics or a rate study.

| Measure | Denominator and result |
| --- | --- |
| First task |1 unique owner+creation key+body;1 accepted original operation;1 physical first prompt;1 completed first turn |
| Creation transport |2 Web submissions (one intentionally dropped receipt, one exact recovery); policy checks also re-read the same accepted intent and separately reject a fresh key. These are not additional tasks. |
| Admission confirmation |54ms from explicit Home submission to observed202 receipt; this intentionally intercepted receipt is not yet user recovery or business completion |
| Duplicate work |0 duplicate native prompts; exactly first/second/third dispatches for3 distinct intended turns;8 model HTTP requests are not8 turns |
| Business outcomes |3/3 transcript turns completed (sequences12,32,46);0 failed/pending/unknown at final authorized read |
| Explicit file action |1 user download,49 exact bytes,242ms;0 failed/unknown user downloads. Separate preview, retained-reader and denied-authorization probes are not additional explicit user downloads. |
| Feedback |1 negative vote and1 reason-set action, both durably read after reload;244ms combined confirmation;0 failed/unknown. Rate denominator is2 actions, not every GET. |
| Suggested follow-ups |1 edited and1 direct send, each one admitted/native-completed turn;1009ms and889ms from send to visible terminal answer. The second includes the deliberate reconnect gate. |
| Generation |3 completed generation observations:2 ready sets,1 empty;0 failed. Elapsed42/23/41ms. Each controlled model response reports30 input,0 cached,15 output tokens; these are synthetic boundary usage, not provider billing. |
| Monetary cost |null/unknown for all3; no invented price, zero-dollar claim or cost-based release qualification |
| Browser |1 actual page/context. Retained reader reports7 captures/99,533 cumulative bytes/0 remaining observers; these counters belong to this context and are not network-total byte measurements. Min interval1000ms, freshness3000ms, max frame262,144 bytes, max observers16; no scale claim. |
| Runtime resources |400 cgroup samples over40.511s; CPU9.96412 core-seconds, peak memory614,567,936 bytes, peak PIDs92. Only native runtime cgroup, excludes host API/Web and other VM jobs. |
| Observed limits |cpu.max=`100000 100000`, memory.max=`2147483648`, pids.max=`512`. Observing requested/recorded quota does not independently prove enforcement. |
| Phone targets/layout |16 measured controls at each of390/320; minimum effective width/height44px. Desktop fine-pointer controls use their documented geometry. Scroll427→427 at1440,528→528 at390,580→580 at320. |

49-byte file SHA256:
`da70a76eb596386fcf3902dcf9292db26e1b71a189afe55674bca82a698f70f4`.

Source files are `relay/journey-metrics.json`, `creation.json`, `web-result.json`,
`public-result.json`, `native-result.json`, `reader-result.json`,
`authority-result.json`, `accessibility-result.json`, and `cgroup-budget.json`.
Deduplicate creation by owner+key+canonical body, follow-ups by source answer and
admitted command/turn, generations by canonical answer/source version, feedback
by answer+version/action and files by explicit action+delivery identity. Retries,
metadata polling and repeated visibility events never enlarge intent denominators.

37 is a separate successful sample:51ms admission,246ms download,242ms combined
feedback,2018ms edited completion,886ms direct completion;3 dispatches/8 model
requests. It is not pooled with38 to imply a production percentile. Earlier failed
attempts remain diagnostic observations, not silently excluded user successes.
The prospective release windows in19-RELEASE.md require new real observations.
