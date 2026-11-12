# Buffered Agent history authorization measurement

Tracks #1793, based on the reviewed #1797 implementation at
`58238ad13201a8dee7b7ce67bb11fb7dc7d651a7`.

The issue's production evidence is limited to one short Agent history in the
2026-11-12 06:46–07:02 UTC window, image `ade94de34`: snapshot n=8, p50 5.56 s,
range 1.67–9.35 s; head n=5, p50 2.63 s, maximum 4.11 s. Those observations include
process stalls and pool contention. Eight authorization snapshots per page and
24 associated Mongo commands were code-derived estimates, not phase-resolved
production measurements. No production long-history measurement was available.

## Local protocol

Use the repository's pinned MongoDB 8.0.13 disposable replica set and the actual
Layer 1 snapshot/head router, Layer 2 authorization/read/binding services, and
Mongo stores. Authentication is supplied by a trusted fixture context; these
measurements exclude Cookie identity, rate-limit and activity middleware.

Prepare 260-row and 1,000-row retained histories, with a command-origin active
turn and a steering operation/message pair whose ledger records pass the normal
validators. Traverse all three or ten 100-row snapshot pages, and capture the
active-turn head. Compare exact serialized response bytes. Warm both modes
first, then alternate mode order for 20 complete traversals per mode, following
the paired method used for #1797. Report per-request p50, nearest-rank p95 and
maximum, with sample counts; keep distinct page command budgets visible.

Observe Mongo command name/collection, physical checkout count, returned BSON
documents/bytes, and the existing `PublicWindowPrefix` scan count/documents/bytes.
The baseline has no authorization phase spans, so the test observer derives
fixed logical phases from command metadata: ancestry aggregate, covering-share
find and abort are authorization; command-ledger reads are binding; remaining
queries are log reads. It retains no query filters, subjects or payload values.
Returned BSON bytes are not compressed wire traffic or server disk-read bytes.
An indexed complete Agent-history scan is distinct from a Mongo collection scan.

First measure the baseline with the page behavior unchanged and
`CHRONO_ACCESS_SNAPSHOT_SESSION_ENABLED` independently off/on. Apply #1793's
implementation gate to the unchanged default path: authorization commands must
account for at least 50% of commands per page. If they do not, deliver only the
shared-snapshot tail-proof change and seek the issue owner's next decision.
After implementing the combined reader, repeat paired measurements for both
switches independently and together. Keep #1648's bounded-history setting fixed
within every comparison; do not credit full-scan removal to this work.

## Required local and integration evidence

The enabled personal-owner path must use exactly three authorization snapshots:
entry authorization, current Agent capture, and final release validation.
Organization/shared callers retain their existing entry membership decisions.
Verify exact response parity, cursor freezing across concurrent append, response
and steering bindings, tool-output preference, and an unchanged disabled path.
Verify share revocation, Agent deletion, owner changes and nonrenewing expiry
between reading and release. Dependency failure and timeout must still pass the
final fence. Error bodies must not disclose private dependency details.

The page store contract must prove current lineage, consecutive sequences and
absence beyond the committed head within one read snapshot. Cover untrusted
legacy, prepared legacy and segmented formats, including malformed, quarantined,
missing-row and uncommitted-tail refusals. Full-scan work must not increase.
Retain existing SSE fill/release behavior and its tests.

Record formatting, clippy, focused nextest, doctests, real-Mongo contracts,
configuration/infra agreement, file-size, secret scanning and manifest validation.
Independent authorization/revocation review and dedicated PR CI are integration
gates. Local results alone do not authorize activation or close the production
verification requirements of #1793.

## Operator handoff

Ship `CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED=false`. Its independent
snapshot-session companion retains its own default-off setting. The release
owner first confirms disabled behavior on the candidate image, then enables the
page switch in staging and runs the revocation/error/integrity cases above,
including an on-to-off rollback rehearsal. Require written authorization and
revocation security review before production activation.

Use the same image and workload settings for production off/on windows, each at
least 30 minutes with natural page opens. Record UTC/SGT bounds, image digest,
source SHA, replica/pool configuration, readiness age, exact feature flags, and
sample counts. Do not generate messages or synthetic traffic in production.
Separate this activation from #1648's bounded-history rollout. Measure snapshot
and head route p50/p95/max, command/checkout distributions, authorization
snapshots and commands per request, error rates, dropped diagnostics, complete
history scans and scanned bytes. Repeat the attribution after bounded-history
activation, because the log-read share changes.

Compare the measured results with #1538's management-read p95 <=500 ms and
p99 <=2 s objectives, while explicitly reporting missing or insufficient samples.
This change does not establish those objectives by itself. Any revocation or
response inconsistency requires immediate rollback to false on the same image.
No durable-format migration is involved. The lead/operator posts production
results to #1793 and #1475; production evidence remains pending until then.
