# Bounded title-transcript discovery

Part of #1349; advances #1320. This compatible consumer does not enable log
segments or complete the transcript, projection and recovery program.

Automatic title discovery and title-input reads currently load the complete
Agent log. The title policy needs the exact first completed task turn, excluding
the setup turn when recorded as such. The unchanged domain fold returns as soon
as that turn completes. A proven prefix containing that terminal event therefore
produces the same title input as the complete history.

With `CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED=true`, for an Agent in
`CHRONO_AGENT_LOG_ADMISSION_AGENTS` with more than 200 entries,
the reader uses #1591's shared `read_proven_agent` interface in the title read's
existing Mongo snapshot. It requires the same captured Agent and trusted proof,
then reads sequences 1–200 through the unique Agent/sequence index. Requested
rows must have valid codecs, consecutive identities and matching creation
lineage. It retains at most 4 MiB of prefix BSON before requesting the existing
complete fallback; one just-read document and driver batch can exceed that
retained-prefix budget. The common proof also reads the validated tail.

The optimization returns only a completed first-task transcript. A long first
turn, a setup/task boundary outside the prefix, an oversized prefix, or an
unfinished/ineligible first turn uses the complete reader and unchanged fold.
No input is silently truncated beyond the existing domain title-input policy,
and a later turn never replaces the first task turn. Smaller histories and
Agents outside the cohort take the original path without an extra proof read.

Missing, stale or unsupported metadata falls back to complete validation.
Corrupt, ahead-of-head or quarantined proof, invalid requested rows and missing
tail evidence refuse the read. Both bounded and complete reads perform no writes,
preparation, repair, audit or transaction lifecycle operations. Complete fallback
uses the shared read-only proof and plain complete traversal, preserving structural,
physical-capacity and creation-lineage validation. Existing quarantine is honored;
only explicit audits and writers may persist a new quarantine marker. As with the shared
public-window proof, immutable rows outside the requested prefix and validated
tail depend on the provider's writer/audit contract; this is not a fresh lifetime
scrub on every read.

## Consumer boundaries

| Consumer | Evidence after this slice |
| --- | --- |
| Title candidates and title-input reads | Fixed proven prefix when sufficient; unchanged complete fallback otherwise. |
| Title claim, publication and unknown-commit reconciliation | Existing complete transactional/receipt evidence. |
| Public event/SSE windows and checkpoint message-assembly suffix | #1634's requested page/range using the same trusted metadata. |
| `AgentReadService::complete_log`, transcript and usage projections | Complete prefix and domain fold remain. |
| Pending-approval snapshots | Complete outstanding-approval fold remains. |
| Worker/conversation publication, runtime-loss and retained recovery | Existing complete proofs and exact receipts remain. |

This is a scoped consumer table, not certification that every segmented
foreground path is bounded. Full public transcript pagination, incremental
usage/current-state projections, long-turn support and segmented recovery remain
open under #1349 and its prerequisites.

## Activation and verification

`CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED` defaults to `false` in the parser and
API manifest and accepts only the exact strings `true` or `false`. It is
independent of the existing default-empty admission cohort, so an already-active
admission/public-window cohort does not silently activate title reads on upgrade.
Preparation and cohort selection remain owned by #1591's release procedure.
The release owner qualifies admission and public windows first, then enables
title reads for that fixed cohort. Setting the title switch to `false` and
restarting the API restores complete title reads without disabling admission or
public windows. All metadata, quarantine and source history are preserved.
This adds no index, document format, projection, backfill or writer. It is a
compatible read optimization under the [migration standard](CMA_Data_Migrations.md),
with no new data emission or reader-floor change.

The release owner records source/image, cohort, pool/worker settings and matched
observation windows. Compare `title_discovery` complete scans, consumed rows/bytes
and scan time with checkout count/wait tails and HTTP latency/503s. Record eligible
title-read sample counts and fallback frequency; fleet gains cannot be inferred
from a fixture or from the historical caller percentage. Missing samples remain
unverified. Stop cohort expansion for output mismatch, integrity refusal, growing
fallback load or availability regression, and disable the title switch. The
shared cohort rollback remains available for a shared proof failure.
No production input, title write or cohort activation is performed
by this implementation's verification.

Real-Mongo tests compare the exact domain input against full reads at 1,000 and
10,000 entries, count all proof/prefix checkouts and consumed log rows, and cover
setup exclusion, the 200-entry boundary, byte fallback, untrusted/corrupt evidence,
an outside-prefix gap on fallback and a concurrent append after snapshot capture.
Transactional title regression tests retain their original coverage.

## Local measurements and limits

Mongo 8.0.13 CMAP counters, with approximately 4 KiB messages in later turns,
include the original title-envelope read, all shared proof queries and the
prefix/full cursor. Both paths return identical domain title input.

| Retained entries | Complete-reader checkouts | Successful-prefix checkouts | Complete-reader log documents | Prefix/proof log documents |
| --- | ---: | ---: | ---: | ---: |
| 1,000 | 6 | 5 | 1,001 | 201 |
| 10,000 | 8 | 5 | 10,001 | 201 |

Fallback costs remain material. At 1,000 entries, a task ending at sequence 201
or 600, or an oversized prefix, used **10 checkouts**: the attempted prefix and
proof precede the original complete reader. With the title switch or cohort
off, the same prepared history keeps its original six checkouts. Missing or
unsupported metadata used five, and stale metadata used seven; those include the
extra attempted proof and are different metadata states from the six-checkout
prepared baseline. A 200-entry history bypasses the shortcut entirely.

In the 1,000-entry fixture, if all unsuccessful attempts cost ten checkouts,
more than 80% of attempts must use the successful prefix to reduce checkout count
overall. Production qualification must measure its actual history/fallback mix;
CPU/decoded-byte improvements and checkout improvements are separate outcomes.
These fixtures do not establish fleet savings or a production latency forecast.

Validation: formatting, cma tests/all-features clippy, 26 existing title-domain
tests, the strict switch parser test and the real-Mongo title contracts are required.
The Mongo contracts include bounded-consumer tests, existing claim and
unknown-commit cases, and task-title authorization/setup recovery. Corrupt complete
fallback tests cover missing/stale/unsupported metadata, both disabled gates,
prefix/byte fallback, lineage, codec, capacity and shared-proof refusal. Both title
input and candidate reads must leave the Agent document exactly unchanged and emit
no write/transaction-control command. Executed results are recorded in the PR;
hosted CI is separate.
