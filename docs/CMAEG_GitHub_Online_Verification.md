# GitHub → CMAEG → CMA → chrono-goals online acceptance

This runbook supersedes the static-proof, execution-group, manual status-PR merge,
App-private-key and automatic-reconciliation instructions dated 2026-11-04.
The installation-authority PR alone cannot execute business deliveries. Release
only after ordinary dispatch and the separate CMA queue fix are merged and reviewed.
The chrono-goals implementation is a separate reviewed PR into `main`.

No step below is evidence that production is already configured or verified.
Record observations against the deployed version. The planner reviews each PR;
merge only after their explicit approval and green CI. Report each CMA merge SHA
so the owner can request Shining's release. Merged is not deployed.

## 1. Read-only release and identity checks

Read the current gateway OpenAPI and authenticated capabilities before writes.
The deployment must expose the existing GitHub operations plus binding verification
and App retirement. Require:

- GitHub `store_ready: true`, `ownership_verification: true`, and, after the dispatch
  PR, `durable_dispatch: true`.
- `automatic_reconciliation: false`; this is the selected release contract.
- Live channel discovery showing routed admission/recovery, durable Custom
  execution, explicit non-human acceptance and CMA provider response replay.
- The deployed CMA queue fix and the intended bounded queue policy for both
  automation consumers. Do not infer it from `busy_message_queue` alone.

If any prerequisite is absent, record the live response and stop the dependent
online steps. Do not supply static configuration, edit the database, restart an
active Sandbox or bypass a feature gate.

Read `GET /api/v1/organizations` on the current CMA origin (currently documented
as `https://cma.nyxid.ai`) using approved authentication. Resolve **ChronoAI's
NyxID organization ID**. It is not GitHub account ID `253878174`. Retain the exact
ID and current admin membership evidence. Every CMAEG management request below
uses `X-CMAEG-Owner: <that organization ID>` unless explicitly retiring the
personal draft.

When NyxID returns a write-approval request, stop and give the planner the exact
method, URL/resource, sanitized request and request ID. The owner approves it.
An approval is not an HTTP commit receipt; recover an uncertain write with its
original body/key and public reads.

## 2. Organization App and personal-draft retirement

The enrollment facts supplied by the owner are:

| Item | Value |
| --- | --- |
| GitHub App | `5606750` (`chronoai-cma-events`) |
| Installation | `178446930` |
| GitHub organization/account | `ChronoAIProject`, `253878174` |
| Initial event repository | `ChronoAIProject/cma`, `1330597394` |
| Personal draft to retire | `gha_4fb8a3bebf2f4c47a7232117ea5e2fb6` |

Use stable operation keys and retain their original request bodies. In organization
scope, register with `POST /v1/github/apps`:

```json
{"app_id":5606750,"display_name":"ChronoAI CMA Events"}
```

Record the returned `app_ref`, version and webhook URL. Initial status must be
`pending_verification`. Do not reuse the personal draft as an organization resource.

Human action: the owner generates/enters the webhook secret through a protected
local client with hidden input and submits webhook-secret PUT with the current
`expected_version`. They enter the same value and the returned URL in GitHub App
settings. Secrets never enter chat, prompts, command arguments, repository files
or diagnostic output. Do not upload an App private key.

Human GitHub App checklist:

- Permissions: Metadata read, Issues read, Pull requests read.
- Subscriptions: Issues, Pull request, **Repository**, and installation lifecycle
  deliveries (`installation`, `installation_repositories`).
- Installation belongs to this App and GitHub account; authorized repositories
  include ID `1330597394`.
- Send a signed ping to the organization endpoint. Expect 200 ignored, no tasks;
  query both receipt-ID and original-delivery-ID lookup paths.

Using **personal scope**, GET the draft and submit
`POST /v1/github/apps/gha_4fb8a3bebf2f4c47a7232117ea5e2fb6/retirement` with its current
version and a stable key. Read back `retired`. Existing ping receipts remain
readable; subsequent webhook requests, including ping, must receive typed 410 and
create no receipt. No verification, rotation or binding operation can reactivate
it. Any existing personal installation approval becomes historical and cannot
authorize work. The organization independently requests and approves the same
installation under a new binding ID; verify the personal owner still reads its old
binding/receipts and cannot read the organization's replacements. Return to
organization scope for subsequent work.

## 3. Independent App and installation approval

Human action: the owner holding `cma-eg:admin` independently verifies App control
and records an evidence reference through App verification POST:

```json
{"expected_version":2,"evidence_ref":"<owner-approved evidence reference>"}
```

Use the actual current version, not the illustrative number. Read back `verified`
and the verifier audit. Signed ping alone is not this approval.

In organization scope, request the installation binding:

```json
{"app_ref":"<organization app_ref>","installation_id":178446930,"account_id":253878174,"repository_ids":[1330597394]}
```

Expect `pending_verification`, no effective repository authority. Human action:
independently confirm installation/App/account/organization ownership and submit
binding verification with the current version:

```json
{"expected_version":1,"evidence_ref":"<owner-approved installation evidence>","repository_ids":[1330597394]}
```

Read back active status, authority revision, approved/effective repository IDs,
empty exclusions and verifier/time/evidence. Verify that personal scope cannot
read the organization binding. Approval has no expiry; later positive lifecycle
events cannot restore authority tightened by suspension/removal. Re-approval is
an explicit administrator action. There is no periodic GitHub API reconciliation.

## 4. Prove the actual organization Agent and execution access

Use a supported **organization-aware provider-binding creation path** with an
**empty/custom-directory Workspace**. chrono-goals is an internal repository; do
not submit its URL to public-repository clone admission. Ordinary
Agent creation does not expose an organization-owner field; supplying a header
or a personal Agent ID is insufficient. Preserve any historical personal pilot
resource and do not reuse it merely because its display name matches.

After the chrono-goals PR is merged, run its pinned standalone bootstrap inside
that Agent's real worktree (Node, npm, local git and managed gh must be available):

```sh
set -eu
source_sha="$(gh api repos/ChronoAIProject/chrono-goals/commits/main --jq '.sha')"
bootstrap_dir="$(mktemp -d)"
gh api --method GET --header 'Accept: application/vnd.github.raw+json' \
  "repos/ChronoAIProject/chrono-goals/contents/scripts/bootstrap-workspace.mjs?ref=$source_sha" \
  > "$bootstrap_dir/bootstrap-workspace.mjs"
node "$bootstrap_dir/bootstrap-workspace.mjs" \
  --ref "$source_sha" --destination "$PWD/chrono-goals"
cd chrono-goals
export SYNC_GITHUB_TRANSPORT=gh
npm run preflight:github
```

The bootstrap needs only Node built-ins and managed gh before dependency setup.
It downloads the pinned tree/blobs, uses Git only for local init/add/commit, runs
`npm ci`, and records `.cache/source-snapshot.json`. Existing destinations are
refused. Refresh uses the same materializer on fresh main. Verify the recorded
commit/repository, current directory and readable sync skill; make the Agent's
instructions explicitly point to that skill in the downloaded working copy.
Npm registry HTTPS is a separate runtime network prerequisite. A successful local
bootstrap is not proof of runtime egress; if `npm ci` is blocked, repair authorized
runtime networking rather than providing Git/npm credentials.

After lifecycle completion, read back and retain evidence proving:

1. The Sandbox resource policy/ownership and Agent's owner attribution identify
   the exact ChronoAI NyxID organization, with management access for the owner.
2. The Agent's Workspace/Worktree contains `ChronoAIProject/chrono-goals` at a
   recorded reviewed revision. The current sync skill is available to that Agent.
3. Managed `gh api` resolves the actual GitHub connection identity. The runtime
   preflight can read Issues/PRs for **every** current `projects.yaml` repository
   and the target board, including current wishes synchronization.
4. In that same runtime, `SYNC_GITHUB_TRANSPORT=gh npm run preflight:github`, a real
   full `npm run sync`, `npm run check` and `npm run build` succeed. A laptop run
   is not evidence. Missing or partial source access must fail rather than publish
   a reduced board.
5. A managed status PR can actually be written and merged through that Agent's
   NyxID GitHub authority. A metadata `push` flag alone is not proof of permitted
   write/merge requests or approval policy.

Never read or forward raw credentials. The runtime's `GH_TOKEN` is an inert
transport placeholder; preserve the managed gh environment, and do not copy it
or a NyxID Agent Key into `SYNC_TOKEN`. Native authenticated Git push is not
provided by this transport.

## 5. Configure both channels and the external schedule

Create the GitHub channel in the same organization scope, using the live supported
durable authoring contract. Its source is:

```json
{"type":"github","app_ref":"<organization app_ref>","installation_ids":[178446930],"repository_ids":[1330597394],"events":["issues","pull_request"],"actions":["opened","edited","closed","reopened","deleted","transferred","labeled","unlabeled","assigned","unassigned","milestoned","demilestoned","ready_for_review","converted_to_draft"],"prompt_template":"Run the installed chrono-goals sync skill for a complete projects.yaml sync, validate, build and publish using its managed publication command. Trigger facts: {{data}}"}
```

The approval is the authority boundary; this source only selects routing within
it. Explicitly accept non-human events and use the recorded existing Agent ID.
No source issue-comment reply is needed. Do not include chrono-goals as a business
event source, which would create publication feedback.

Create a second **durable Custom** channel in the same owner scope, with HMAC auth,
explicit non-human acceptance and a full-sync prompt. Use
`durable_custom_execution: true` and the current Custom contract. Read back both
channel definitions and provider associations to prove that they bind to the
**same exact Agent ID**, not two similarly named Agents or two profile-created
conversations. Check integration/webhook/binding/source diagnostics dynamically.

Human action: in chrono-goals → Settings → Secrets and variables → Actions, set
repository variables `CMAEG_SYNC_ENDPOINT` (the Custom channel event URL) and
`CMAEG_SYNC_KEY_ID` (its configured HMAC key ID), plus repository secret
`CMAEG_SYNC_HMAC_SECRET`. These are consumed by `.github/workflows/sync.yml`. The secret's configured base64 string is
used as UTF-8 HMAC key bytes; **do not base64-decode it**. Sign the exact structured
CloudEvent using the protocol's Standard Webhooks envelope. The workflow runs near
00:55 UTC and exposes `workflow_dispatch`; missing configuration must fail clearly.

The owner asks loning to stop the former daily writer when the replacement is live.
Do not send that message on the owner's behalf or silently allow two status writers.

## 6. Online acceptance and trace

Use an authorized real cma issue/PR change. Keep each stage's actual observation:

| Stage | Required evidence |
| --- | --- |
| GitHub delivery | App/integration, original delivery ID, repository/event/action and HTTP result |
| Receipt | 202, stable receipt ID, pending/dispatched progress, owner and authority revision |
| Admission | `task_ids` contains the durable routed admission ID |
| Agent | Same target Agent ID, response/command/turn identity and terminal outcome |
| Publisher | Machine-readable result and managed PR identity, or proven no-change with no PR |
| Checks and merge | Local check/build success; successful CI tied to exact PR head; merge with head-SHA match; merge commit |
| Pages | Successful post-merge deployment and rendered board showing the expected current fact |

Repeat these acceptance cases:

- Redeliver the identical GitHub delivery: same receipt/task, no second turn.
- Send a filtered action: ignored with no task.
- Make two quick changes while a sync is busy: two separate admissions and serial
  turns on the same Agent; both complete within the configured queue bound.
- Dispatch the schedule workflow: authenticated Custom admission and a run on that
  same Agent. Record the workflow run and Custom event identity.
- Verify a no-change run creates no PR; an existing managed PR is reused on retry;
  an uncertain publication response is recovered without duplicate publication.
- Confirm the Agent merges only its managed status PR after local validation and
  exact-head CI. Failed/missing/stale checks prevent merge. A newer main requiring
  refresh reruns the full sync; unresolved mapping work is reported for continuation.

`Task.result` is retained-but-unused. Never use a model sentence or a populated
legacy result shape as PR/merge evidence. The delivery → receipt → admission →
Agent turn → publisher → PR head → merge commit → Pages trace must be recoverable
from actual public reads and runtime output.

Stop dependent acceptance on a missing capability, wrong owner/Agent, lost receipt,
duplicate turn, partial-source sync, queue expiry, uncertain merge or failed Pages
deployment. Report the exact boundary and sanitized evidence. Lifecycle authority,
Agent GitHub grants and already admitted/running work are separate boundaries;
retiring ingress does not revoke independent outbound credentials.
