# Always-on policy and immutable admission receipts

Migration record `2026-11-16-am-always-on-admission-proof`, issue #2161,
package M5. Agent Manager owns the Sandbox aggregate and admission receipt;
the release owner controls deployment. This change expands retained readers.
It does not transform stored data or introduce a new writer.

## Problem and correction

An origin-bound Sandbox retains its original admission name and fingerprint.
The fingerprint includes the admitted profile's three-state `always_on` value:
absent, false, or true. An authorized later policy change updates the current
profile while preserving that receipt. The old reader incorrectly compares the
receipt with the current policy and rejects the Sandbox and dependent reads.

The retained reader proves the original policy by checking the original complete
fingerprint against the three possible policy values. All other admission
identity fields remain exact: owner, origin, requested name, profile identity,
revision, digest and content, service reaches, credential source, sign-in mode,
and limits. No matching complete fingerprint means corruption. The current
aggregate and its profile content digest must also validate independently.

The original policy is carried only inside the in-memory replay value, separately
from the current Sandbox policy. Service-level replay validation uses that proof
again and returns the current Sandbox. Fresh admission and fresh receipt
serialization still require the exact supplied initial policy. A repeated
creation request with a changed initial policy remains an origin conflict.

## Data and recovery contract

| Required field | Contract |
| --- | --- |
| Phase and owner | Compatible retained-reader correction; Agent Manager; issue #2161. Exact commit, executable hash and test receipts are attached to the completing PR. |
| Affected stores and triggers | Existing Sandbox documents and in-memory admission records. Retained read/replay constructors change. Existing admin approval, owner relinquishment, revision CAS, atomic usage persistence and admission transactions retain their writer behavior. |
| Stored formats | Unchanged Sandbox envelope, profile, admission name and fingerprint. No field, index, validator, TTL, collection or default is added. The proven original policy is not serialized. |
| Preservation | Keep original IDs, name, origin, receipt bytes, policy value, revision, event history, usage watermark, runtime, credential and work authority. Reads never repair or rewrite a row. |
| Reader floor | Readers with this correction support origin-bound rows after policy changes. The predecessor reader fails on that already-supported writer output; baseline RED is retained. Old readers remain unsuitable for those rows during overlap or rollback. |
| Writer gate and cohort | No new writer or activation switch. Existing authorization and expected-revision boundaries continue to govern policy changes. This reader-only change does not authorize production activity. |
| Backfill and startup | None. No scan, startup DDL, migration command, background repair or operator DB write. Per-read proof tries at most three candidate digests, followed by validation of the selected identity. |
| Backup and dry-run | No destructive transformation or deletion. Tests use disposable fixtures; authoritative data is untouched. No new backup step is required by this correction. |
| Rollback and forward recovery | Preserve data and use a compatible reader. Do not remove the current policy or replace admission fingerprints to fit an old binary. If another immutable field is corrupt, return the existing typed corruption failure. |
| Verification | Lifecycle and Trigger origins; all three admitted policy states; enable/revoke; independent store reads and replay; service revalidation; owner relinquishment and authorization; original replay and changed-payload refusal; corruption and revision fencing. Native and exact-image results are separate. |

## Acceptance limits

The constructor proves the original identity from the retained fingerprint. It
does not establish who authorized a policy change; the existing authenticated
mutation path and CAS supply that authority. A policy-only difference is
therefore not treated as immutable admission corruption.

Package tests do not establish the full two-AM feature matrix. Batch 13b still
requires the combined native suite, C275, the full journey and readiness/HPA
qualification on the final image before the approved production transition.
Deployment and production verification must record their own exact image and
observations. No predecessor image compatibility or production PASS is inferred
from source inspection or a package unit test.
