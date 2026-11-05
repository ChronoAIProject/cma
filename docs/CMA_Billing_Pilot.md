# CMA-side chronoai organization billing pilot

This is the owner/operator runbook for [CMA #1322](https://github.com/ChronoAIProject/cma/pull/1322)
and [NyxID #2204](https://github.com/ChronoAIProject/NyxID/pull/2204).
Execution requires the owner's activation approval. This PR does not activate
production billing. The pilot bills **allocated CPU, RAM and retained disk ×
duration**. Measured CPU/RAM/PVC usage settlement is a separate project.

## Configuration and admission scope

Set these on the Agent Manager deployment through the normal deployment process.
Keep the repository's default-off configuration until the activation step.

| CMA key | Pilot value |
| --- | --- |
| `CHRONO_NYXID_SANDBOX_BILLING_ENABLED` | `false` during deployment; `true` after NyxID readiness and the owner's approval |
| `CHRONO_USAGE_BILLING_ENABLED` | `false` throughout deployment, pilot and rollback; NyxID remains the only billing emitter |
| `CHRONO_NYXID_SANDBOX_BILLING_BASE_URL` | The approved HTTPS NyxID API origin followed by `/api/v1/billing/sandboxes/`; no query, credentials or fragment |
| `CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID` | The canonical UUID v4 of the NyxID General-purpose producer service account, equal to NyxID's `CMA_BILLING_SERVICE_ACCOUNT_ID` |
| `CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF` | `NYXID_CMA_BILLING_PRODUCER_TOKEN`, the name of an environment key injected from the approved Secret; never the token value |

Inject that named key into the Agent Manager process with a valid service-account
JWT whose live account and token both have `billing:sandbox:write`. Rotation
requires refreshing the process environment. Retain the existing native
agent-sandbox provider/controller and runtime-image pins. Unsupported provider
deadline capabilities cannot admit paid Sandboxes.

NyxID owns eligibility: `CMA_BILLING_ENABLED=false` is the independent fleet
admission switch. The effective `experimental:cma-sandbox-billing` flag uses
global → organization → personal precedence. Keep the global flag **false**, set
the **chronoai organization only** to true, leave every other organization
override absent or false, and remove or set false any personal true overrides
outside chronoai. Inspect effective flags for both test
accounts; personal false overrides can exclude a chronoai member. Organization
membership selects people whose **personal wallets** pay; it does not make the
organization a payer. Ordinary NyxID billing eligibility, price synchronization,
wallet readiness and entitlement must also pass for pilot participants.

## Deploy and enable in order

1. Deploy NyxID #2204 first with its fleet switch false and the cohort global
   flag false. Upgrade all compatible NyxID readers, billing workers and
   reconciliation paths. Verify the approved rate-card version is synchronized,
   producer authentication is valid, and receipt recovery/evidence/settlement
   remain available while paid admission is disabled. Do not copy prices,
   credentials, wallet details or customer content into GitHub.
2. Verify the final NyxID exclusion contract before CMA activation. Fleet-off
   and non-cohort decisions must be returned from the money-free quote endpoint
   **before** wallet/catalog/provider dependencies. An outage or insufficient
   funds must remain distinct from an explicit exclusion. Run both excluded-user
   cases against the deployed contract; the published predecessor checks some
   paid dependencies first, so it is not sufficient evidence for this gate.
3. Reconcile the existing Agent Manager Mongo runtime role with the new binary's
   `--print-runtime-role` output, following
   [Sandbox funding grants](CMA_Service_Isolation.md#sandbox-funding-grants).
   Deploy CMA #1322 with both billing flags false. Verify readiness and the
   `sandbox_billing_due` index; there must be no schema-authority or Mongo
   Unauthorized errors. Preserve all retained Sandbox/home identities.
4. After owner approval, configure the producer references above and enable
   `CHRONO_NYXID_SANDBOX_BILLING_ENABLED=true` while NyxID's fleet gate remains
   false. Verify a test user can create, pause and resume without a reservation,
   funding deadline or billing-related failure. Keep the direct Lago exporter
   false and verify it has no active delivery worker.
5. Configure only the chronoai organization cohort, audit personal overrides,
   and then enable NyxID's `CMA_BILLING_ENABLED=true`. First use an explicitly
   consenting chronoai organization test user with a ready personal wallet.
   Use a fresh test Sandbox, so an existing unbilled generation remains unbilled.
   In parallel verify an account outside chronoai still creates and resumes
   normally with no `POST /windows`, no hold and no charge.

## One funded Sandbox acceptance

1. Record only sanitized operation/request/window identifiers and rate-card
   **version**, with any sensitive evidence held in the approved operator system.
   Create one fresh Sandbox after consent. Verify `POST /quotes` returns the
   exact owner/Sandbox/allocation and version; a quote alone moves no funds.
2. Verify the exact durable reservation precedes `POST /windows`, produces one
   recoverable hold/receipt, and covers the requested CPU/RAM/disk. Duplicate
   delivery and `GET /receipts/{request_id}` must return the same window, not
   another debit. The native core Sandbox must have the matching exclusive
   `shutdownTime` and `shutdownPolicy: Retain`, with UID/resource-version fencing.
3. Observe the renewal boundary: windows are at most 300 seconds, renew only in
   the final 30 seconds and may shorten at UTC month/rate cutoffs. Every provider
   deadline extension needs its own persisted matching receipt. Do not change a
   rate card during this first acceptance run.
4. Pause the test Sandbox before a funded compute window ends. Verify compute
   teardown precedes settlement intent and its home remains intact. Wait until
   the full window is elapsed and observed; check contiguous retained allocation
   evidence, one frozen submission, NyxID `evidence_accepted=true`, and CMA's
   advancing durable evidence cursor. CPU/RAM final quantities must reflect the
   shorter allocated duration, with unused reserved amounts refunded/released.
   Retained disk remains allocated and may continue disk-only funded windows.
5. With the test owner's explicit deletion authorization, retire the test Sandbox
   to end retained disk allocation. After observed teardown and the final window
   cutoff, verify zero-allocation tails settle the remaining reservation. Verify
   NyxID's receipt and wallet/journal reconcile reserved, finalized and refunded
   quantities exactly, with no duplicate Lago export. A provider HTTP acceptance
   alone does not establish a finalized invoice. Record the final receipt/status
   and the operator's billing acceptance before expanding the cohort.

## Monitoring

During the pilot, monitor Agent Manager readiness, new Sandbox creation/resume
failures, `Sandbox funding reconciliation will retry`, `Sandbox funding sweep
failed`, and `Sandbox generation admitted without billing: outside paid admission`.
For paid test state, inspect sanitized counts/ages of `pending` reservations,
`windows` evidence backlog, `attempts`, `retry_at`, and `last_error`; compare
confirmed cutoffs with native shutdown deadlines. A growing backlog, stalled
cursor, duplicate debit, unexpected outside-cohort reservation or inability to
create/resume an excluded user's Sandbox stops expansion immediately.

Use read-only Kubernetes get/list/logs and approved read-only monitoring GETs.
Do not use Cloud Logging or perform production writes from this verification
task. Stop and report if gcloud reauthentication is requested. Publish only
aggregate results and sanitized technical evidence to GitHub, never secrets,
user content, wallet details or infrastructure cost figures.

## Rollback without stranding holds

1. Have the authorized NyxID operator set `CMA_BILLING_ENABLED=false` to stop new
   paid coverage, and disable the chronoai cohort override. Keep CMA's NyxID
   adapter **true**, its producer credential valid, and the compatible NyxID
   receipt/evidence/settlement workers running while liabilities drain. Keep
   `CHRONO_USAGE_BILLING_ENABLED=false` throughout.
2. Previously excluded generations remain on their ordinary unbilled lifecycle.
   Existing paid generations retain confirmed deadlines; failed renewal pauses
   compute while preserving homes. Pending requests are potentially committed:
   recover/replay their original keys and payloads. Never delete them, replace
   their keys, mark them excluded or manually advance an evidence cursor.
3. Reconcile all confirmed receipts and deliver frozen evidence after the last
   covered interval is observed, including retained disk and actual zero tails.
   Verify no unresolved pending request/hold, no undelivered funded window, and
   acknowledged refunds/settlements. Resolve any gate-off rejection of a pending
   request with the NyxID operator; a 404 alone is not proof that it never committed.
4. Only after the drain is verified, set
   `CHRONO_NYXID_SANDBOX_BILLING_ENABLED=false` and refresh CMA configuration.
   Turning it off earlier also stops recovery/evidence delivery and can strand
   holds; an emergency CMA shutdown therefore requires preserving every record
   and restoring the compatible adapter to finish the drain. Keep schema-aware
   readers and runtime-image pins; do not roll back to a binary that rejects
   already-written billing records. Re-enable only after a new owner-approved
   acceptance run.
