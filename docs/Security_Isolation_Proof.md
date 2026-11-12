# Friday application authorization proof (Part of #1549)

This document defines the Friday application proof and its explicit limits.
Local results, source revision and reproduction commands belong in the proof PR;
production qualification additionally requires candidate-bound release evidence.

This is application-contract evidence, not certification of the complete gate.
The executable route matrix is `api/crates/layer1-public/tests/fixtures/tenant-isolation-matrix.json`.
Its 171 documented operations are checked against OpenAPI; an additional source
mount audit identifies supplemental absolute resource routes, including history export
and the runtime MCP tool endpoint. The scope covers the named resource families,
command receipts and their delegated/binding transports; unrelated profile/catalog,
Bot and Trigger management remain outside this proof slice.

## Proof layers

- `probe`: replay valid owner requests as another capable tenant before the exact
  owner control. Foreign and nonexistent targets must have identical typed denial;
  GET also exercises implicit HEAD. Stateful setup and capture dependencies remain
  in their original order. Subsequent owner results catch changed fixture state.
- `receipt-contract`: Sandbox command receipt aliases pair permitted reads with
  foreign/missing IDs, implicit HEAD and revoked access against the real service.
- `delegation`: CMA broker, current actor-grant checks and actual hierarchy handlers
  prove foreign targets remain hidden and revoked grants stop before dispatch.
- `binding-contract`: binding-authorized conversation/event/media handler projections,
  supplemented by existing CMA binding-origin and credential contracts.
- `contract`: existing exact real-handler owner/foreign or revoked-grant fixtures.
  Sharing fixtures retain real caller contexts; a subject/context mismatch alone
  is not counted as cross-tenant evidence.
- `mongo-report`: real production report store + HTTP metadata, rows, child detail,
  series and operator projections, another principal, parent transfer, adapter reopen
  and body-carried foreign Sandbox ID. No report allocation on refused submission.
- `mongo-service-access`: production consent service and Mongo adapter for all four
  list/approve/deny/replace routes, with both users authorized at the identity boundary.
  A foreign owner cannot mutate state or borrow the owner’s idempotency receipt.
- `mongo-lifecycle`: contextual parent authority/restart contract in
  `agent_lifecycle_workspace_mongo_tests.rs`, plus exact lifecycle HTTP projections.
- `operator`: regular-user denial paired with authorized operator success.
- `filtered-inventory`: owner-scoped empty/filtering results are the denial semantics;
  an authorized empty list is not required to return HTTP 403 or 404.
- `context-creation`: conversation metadata cannot select a foreign workspace or Sandbox.
- `own-creation`: document creation generates an owner-scoped identity; an optional
  origin hint does not grant read/write authority over that Agent.
- `profile-creation`: Sandbox creation has capability/private-profile controls;
  visitor quotas and direct-create policy are owned by #1485.
- `unsupported`: legacy response deletion/cancellation uniformly refuse with 422.
  The terminal route checks ownership before returning its supported typed 501.

Additional contracts: real-Mongo document revoke/reopen/old grant and link receipt
replay, unchanged aggregate after refusal, and owner/foreign/source/revision/list
HTTP reads; actual CMA auth + document handlers for warm-cookie and bearer
revocation, logout, expiry and removed permissions; the merged history test’s
1,000 controlled provider-read/revoke interleavings; merged verification and
expensive-read admission budgets. #1593 reserves remain default-off until the
separate verified-reader activation train.

Runtime MCP adds a paired publish/update control, foreign owner and same-owner
foreign-Agent refusals, absent-ID equivalence, receipt replay and stale capability
denial. Identity transport is doubled here; existing Mongo capability contracts
separately check retained runtime identity, ownership and changed incarnation.

## Friday limits

Keep #1549 open. These tests cannot qualify controller/kernel/network/volume
isolation for cold, warm or resumed runtimes; metadata/private-network/control-plane
reachability; retained-credential cleanup on real deletion/recycling; candidate-image
vulnerability exploitability/reachability; live NyxID organization/scoped-key contracts;
or production outcomes. Local identity transport and runtime I/O doubles are explicit.
Mongo tests exercise the real database contracts, not deployed storage or production
configuration. Read-only production smoke cannot prove a new full-capacity revoke
or a controlled export race. Candidate SHA/digests/configuration/window/N and relayed
release-owner evidence are still required. No `/measure ... green` is asserted.

## Running the proof

Run the `tenant_isolation` tests in `cma`, `layer1-public` and `layer2-core`, then
the existing `rest::history_export_tests`, `document_artifacts`, sharing, command
admission, authorization and expensive-read regressions. The shared replay keeps
every setup/capture dependency and compares immutable expected owner responses.
The new generic probes cover 73 allowed requests across 52 operations, issuing
208 foreign/absent requests including implicit HEAD; the route matrix also names
the separate receipt, delegation, runtime, inventory and database contracts.

The ignored Mongo tests require an owned disposable replica set at the repository's
pinned MongoDB 8.0.13 image. Run the `tenant_isolation`, document artifact,
resource-access, runtime capability and lifecycle Mongo contracts against it.
`.github/scripts/mongo-contract-tests.sh` includes the new tests in CI and owns
fixture creation/cleanup; it never accepts an existing production database.
Keep the local heavy-build lock, two-job limit and disk floor from the sprint
contract. Format, warnings-denied Clippy and the runner's selection self-tests
are additional gates. Local success does not replace exact production evidence.

## Route matrix

Each case ID resolves to a checked-in public contract fixture. Source/Mongo proofs
above supplement ports that intentionally use HTTP projection doubles.

| Operation | Allowed/control case | Denied case | Proof |
| --- | --- | --- | --- |
| `DELETE /api/v1/agents/{id}` | `agent-delete` | `agent-delete-not-stopped-denied` | probe |
| `DELETE /api/v1/agents/{id}/browser` | `browser-delete-success` | `browser-delete-foreign` | contract |
| `DELETE /api/v1/agents/{id}/files` | `filesystem-directory-delete` | `filesystem-delete-foreign` | contract |
| `DELETE /api/v1/agents/{id}/share-links/{share_id}` | `typed-link-revoke` | `typed-link-revoke-not-owner` | contract |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | `binding-conversation-delete` | `binding-delete-conversation-forbidden` | binding-contract |
| `DELETE /api/v1/documents/{id}/comments/{comment_id}` | `document-uncomment-success` | `document-uncomment-foreign` | probe |
| `DELETE /api/v1/documents/{id}/share-links/{link_id}` | `document-unlink-success` | `document-unlink-foreign` | probe |
| `DELETE /api/v1/documents/{id}/shares/{subject}` | `document-revoke-success` | `document-revoke-foreign` | probe |
| `DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}` | `share-revoke-success` | `share-revoke-not-found` | contract |
| `DELETE /api/v1/responses/{response_id}` | `responses-delete-refused` | `` | unsupported |
| `DELETE /api/v1/sandboxes/{id}` | `state-sandbox-retire-before-pause` | `sandbox-delete-denied` | probe |
| `DELETE /api/v1/workspaces/{id}` | `workspace-live-agent-removal` | `workspace-live-agent-conflict-denied` | probe |
| `GET /api/v1/admin/runtime-inventory` | `admin-runtime-inventory` | `admin-runtime-inventory-denied` | operator |
| `GET /api/v1/admin/sandboxes` | `admin-sandbox-list` | `admin-sandbox-list-denied` | operator |
| `GET /api/v1/admin/sandboxes/{id}` | `admin-sandbox-read` | `admin-sandbox-read-denied` | operator |
| `GET /api/v1/admin/sandboxes/{id}/events` | `admin-sandbox-events` | `admin-sandbox-events-denied` | operator |
| `GET /api/v1/admin/usage` | `usage-admin-summary-success` | `usage-admin-summary-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}` | `metrics-1--report-ready` | `metrics-1--report-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations` | `metrics-1--report-owner-allocations-ready` | `metrics-1--report-owner-allocations-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}/owners` | `metrics-1--report-owners-ready` | `metrics-1--report-owners-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes` | `metrics-1--report-sandboxes-ready` | `metrics-1--report-sandboxes-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes/{sandbox}` | `metrics-1--report-sandboxes-sandbox-ready` | `metrics-1--report-sandboxes-sandbox-forbidden` | operator |
| `GET /api/v1/admin/usage/metrics/reports/{report}/series` | `metrics-1--report-series-ready` | `metrics-1--report-series-forbidden` | operator |
| `GET /api/v1/admin/usage/sandboxes` | `usage-admin-sandboxes-success` | `usage-admin-sandboxes-forbidden` | operator |
| `GET /api/v1/agent-lifecycle/{id}` | `lifecycle-read-owner` | `lifecycle-read-foreign` | mongo-lifecycle |
| `GET /api/v1/agents` | `agent-list` | `agent-list-denied` | filtered-inventory |
| `GET /api/v1/agents/provenance` | `channel-inventory-owner-active` | `creation-inventory-parent-refused` | filtered-inventory |
| `GET /api/v1/agents/{id}` | `agent-read` | `shared-agent-revoked-detail` | probe |
| `GET /api/v1/agents/{id}/access-requests` | `agent-access-list-success` | `agent-access-list-forbidden` | mongo-service-access |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables` | `deliverables-owner-list` | `deliverables-foreign-list` | contract |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}` | `deliverables-owner-read-document` | `deliverables-foreign-read-document` | contract |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download` | `deliverables-owner-download-document` | `deliverables-revoked-download-document` | contract |
| `GET /api/v1/agents/{id}/artifacts` | `artifact-owner-list` | `artifact-revoked-list` | contract |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}` | `artifact-owner-read` | `artifact-revoked-read` | contract |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}/download` | `artifact-owner-download` | `artifact-revoked-download` | contract |
| `GET /api/v1/agents/{id}/browser` | `browser-get-success` | `browser-get-foreign` | contract |
| `GET /api/v1/agents/{id}/browser/control` | `browser-control-read` | `browser-control-get-foreign` | contract |
| `GET /api/v1/agents/{id}/channel-usage` | `channel-usage-owner` | `channel-usage-revoked` | contract |
| `GET /api/v1/agents/{id}/channel-usage/history` | `channel-usage-history-first` | `channel-usage-revoked-history` | contract |
| `GET /api/v1/agents/{id}/desktop` | `desktop-discovery` | `desktop-discovery-other_owner` | contract |
| `GET /api/v1/agents/{id}/desktop/ws` | `desktop-production-origin-owner` | `desktop-production-origin-foreign-owner` | contract |
| `GET /api/v1/agents/{id}/environment` | `environment-owner` | `environment-foreign` | contract |
| `GET /api/v1/agents/{id}/events` | `agent-events` | `shared-agent-revoked-events` | probe |
| `GET /api/v1/agents/{id}/files/content` | `filesystem-content-owner-success` | `filesystem-content-foreign` | contract |
| `GET /api/v1/agents/{id}/files/download` | `filesystem-download-owner-success` | `filesystem-download-foreign` | contract |
| `GET /api/v1/agents/{id}/files/tree` | `filesystem-tree-owner-success` | `filesystem-tree-foreign` | contract |
| `GET /api/v1/agents/{id}/message-feedback` | `message-feedback-initial` | `message-feedback-list-foreign` | contract |
| `GET /api/v1/agents/{id}/messages/{message_id}/feedback` | `feedback-reasons-empty` | `feedback-reasons-get-foreign` | contract |
| `GET /api/v1/agents/{id}/models` | `agent-models-owner` | `agent-models-foreign` | probe |
| `GET /api/v1/agents/{id}/preferences` | `agent-preferences-initial-read` | `agent-preferences-read-forbidden` | probe |
| `GET /api/v1/agents/{id}/provenance` | `agent-provenance-owner` | `agent-provenance-revoked` | contract |
| `GET /api/v1/agents/{id}/provider-associations` | `provider-associations-owner` | `provider-associations-revoked` | contract |
| `GET /api/v1/agents/{id}/resources` | `agent-resources` | `shared-agent-revoked-resources` | probe |
| `GET /api/v1/agents/{id}/settings` | `agent-settings-initial-read` | `agent-settings-read-forbidden` | probe |
| `GET /api/v1/agents/{id}/share-links` | `typed-link-list` | `typed-link-list-not-owner` | contract |
| `GET /api/v1/agents/{id}/share-recipients` | `typed-link-search` | `typed-link-search-not-owner` | contract |
| `GET /api/v1/agents/{id}/skills` | `agent-skills-read-owner` | `agent-skills-read-foreign` | probe |
| `GET /api/v1/agents/{id}/source` | `source-manual` | `source-foreign` | probe |
| `GET /api/v1/agents/{id}/suggestions` | `suggestions-owner` | `suggestions-foreign` | contract |
| `GET /api/v1/agents/{id}/terminal` | `terminal-unsupported` | `terminal-missing` | probe |
| `GET /api/v1/agents/{id}/transcript` | `agent-transcript` | `shared-agent-revoked-transcript` | probe |
| `GET /api/v1/agents/{id}/usage` | `agent-usage` | `shared-agent-revoked-usage` | probe |
| `GET /api/v1/agui/agents/{id}/sandbox` | `agui-sandbox-context` | `agui-context-foreign` | contract |
| `GET /api/v1/agui/events` | `agui-events-replay` | `agui-events-foreign` | filtered-inventory |
| `GET /api/v1/agui/launches/{id}/events` | `launch-review-stream` | `launch-missing-stream` | probe |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | `binding-inspection-mapped` | `binding-inspection-forbidden` | binding-contract |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}` | `binding-event-get` | `binding-event-owner-refused` | binding-contract |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment` | `binding-media-receipt` | `binding-media-receipt-forbidden` | binding-contract |
| `GET /api/v1/conversations/{id}` | `responses-conversation-read` | `responses-conversation-foreign` | contract |
| `GET /api/v1/conversations/{id}/items` | `responses-items-first-page` | `responses-items-foreign` | contract |
| `GET /api/v1/desktop-capability` | `desktop-capability-owner-200` | `desktop-capability-forbidden-403` | mongo-lifecycle |
| `GET /api/v1/documents` | `document-list-success` | `document-list-forbidden` | filtered-inventory |
| `GET /api/v1/documents/{id}` | `document-read-success` | `document-read-foreign` | probe |
| `GET /api/v1/documents/{id}/comments` | `document-comments-success` | `document-comments-foreign` | probe |
| `GET /api/v1/documents/{id}/revisions` | `document-revisions-success` | `document-revisions-foreign` | probe |
| `GET /api/v1/documents/{id}/share-links` | `document-links-success` | `document-links-foreign` | probe |
| `GET /api/v1/documents/{id}/shares` | `document-shares-success` | `document-shares-foreign` | probe |
| `GET /api/v1/documents/{id}/source` | `document-source-success` | `document-source-foreign` | probe |
| `GET /api/v1/resource-access/{kind}/{id}` | `share-effective-success` | `share-effective-not-found` | contract |
| `GET /api/v1/resource-shares/{kind}/{id}` | `share-list-success` | `share-list-not-found` | contract |
| `GET /api/v1/resource-shares/{kind}/{id}/{share_id}` | `share-read-success` | `share-read-not-found` | contract |
| `GET /api/v1/responses/{response_id}` | `responses-read-completed` | `responses-read-foreign` | contract |
| `GET /api/v1/sandbox-commands/{id}` | `sandbox-command-shared-recover` | `sandbox-command-missing-receipt` | receipt-contract |
| `GET /api/v1/sandboxes` | `sandbox-list` | `sandbox-list-denied` | filtered-inventory |
| `GET /api/v1/sandboxes/{id}` | `sandbox-read` | `shared-hierarchy-0-revoked` | probe |
| `GET /api/v1/sandboxes/{id}/events` | `sandbox-events` | `sandbox-events-denied` | probe |
| `GET /api/v1/sandboxes/{id}/workspaces` | `workspace-list` | `shared-hierarchy-1-revoked` | probe |
| `GET /api/v1/usage` | `usage-summary-success` | `usage-summary-forbidden` | filtered-inventory |
| `GET /api/v1/usage/metrics/reports/{report}` | `metrics-0--report-ready` | `metrics-0--report-forbidden` | mongo-report |
| `GET /api/v1/usage/metrics/reports/{report}/sandboxes` | `metrics-0--report-sandboxes-ready` | `metrics-0--report-sandboxes-forbidden` | mongo-report |
| `GET /api/v1/usage/metrics/reports/{report}/sandboxes/{sandbox}` | `metrics-0--report-sandboxes-sandbox-ready` | `metrics-0--report-sandboxes-sandbox-forbidden` | mongo-report |
| `GET /api/v1/usage/metrics/reports/{report}/series` | `metrics-0--report-series-ready` | `metrics-0--report-series-forbidden` | mongo-report |
| `GET /api/v1/usage/sandboxes` | `usage-sandboxes-success` | `usage-sandboxes-forbidden` | filtered-inventory |
| `GET /api/v1/usage/sandboxes/{id}` | `usage-sandbox-success` | `usage-sandbox-cross-owner` | probe |
| `GET /api/v1/usage/sandboxes/{id}/events` | `usage-events-success` | `usage-events-cross-owner` | probe |
| `GET /api/v1/workspaces/{id}` | `workspace-read` | `shared-hierarchy-2-revoked` | probe |
| `GET /api/v1/workspaces/{id}/worktrees` | `worktree-list` | `shared-hierarchy-3-revoked` | probe |
| `GET /api/v1/worktrees/{id}` | `worktree-read` | `shared-hierarchy-4-revoked` | probe |
| `GET /api/v2/agents/{agent_id}/events` | `agent-events-v2-tail-rich` | `agent-events-v2-events-foreign` | contract |
| `GET /api/v2/agents/{agent_id}/events/head` | `agent-events-v2-head-rich` | `agent-events-v2-head-foreign` | contract |
| `GET /api/v2/agents/{agent_id}/events/snapshot` | `agent-events-v2-snapshot-rich` | `agent-events-v2-snapshot-foreign` | contract |
| `GET /api/v2/agents/{agent_id}/queued-inputs` | `queued-input-v2-owner-list` | `queued-input-v2-foreign-list` | contract |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/controls` | `interruption-v2-observe` | `response-controls-v2-foreign-observe` | contract |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | `queued-cancellation-v2-owner-observe` | `queued-cancellation-v2-foreign-observe` | contract |
| `GET /api/v2/commands/{command_id}` | `response-controls-v2-cancel-status` | `response-controls-v2-foreign-status` | contract |
| `GET /api/v2/conversations/{conversation_id}` | `responses-v2-conversation` | `responses-v2-conversation-foreign` | contract |
| `GET /api/v2/conversations/{conversation_id}/items` | `responses-v2-items` | `responses-v2-items-foreign` | contract |
| `GET /api/v2/responses/{response_id}` | `responses-v2-retrieve` | `responses-v2-retrieve-foreign` | contract |
| `GET /api/v2/responses/{response_id}/events` | `responses-v2-replay` | `responses-v2-replay-foreign` | contract |
| `PATCH /api/v1/agents/{id}/preferences` | `agent-preferences-update` | `preferences-agent-input-unavailable-denied` | probe |
| `PATCH /api/v1/agents/{id}/settings` | `agent-settings-update` | `settings-agent-input-unavailable-denied` | probe |
| `PATCH /api/v1/documents/{id}` | `document-update-success` | `document-update-foreign` | probe |
| `PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}` | `share-update-success` | `share-update-not-found` | contract |
| `POST /api/v1/admin/sandboxes/{id}/always-on` | `auto-pause-admin-always-on` | `auto-pause-admin-forbidden` | operator |
| `POST /api/v1/admin/sandboxes/{id}/retire` | `admin-sandbox-retire-success` | `admin-sandbox-retire-missing-denied` | operator |
| `POST /api/v1/admin/sandboxes/{id}/runtime-update` | `admin-runtime-update-noop` | `admin-runtime-update-forbidden` | operator |
| `POST /api/v1/admin/usage/metrics/options` | `metrics-options-owners` | `metrics-options-forbidden` | operator |
| `POST /api/v1/admin/usage/metrics/reports` | `metrics-1-create-ready` | `metrics-1-create-forbidden` | operator |
| `POST /api/v1/agent-delegations/identity` | `agent-delegation-identity-linked` | `agent-delegation-identity-refused` | delegation |
| `POST /api/v1/agent-delegations/operations` | `agent-delegation-operations-organizations` | `agent-delegation-operations-wrong-owner` | delegation |
| `POST /api/v1/agent-share-links/{share_id}/redeem` | `typed-link-redeem` | `typed-link-redeem-wrong-recipient` | contract |
| `POST /api/v1/agents` | `desktop-create-false` | `lifecycle-create-forbidden` | mongo-lifecycle |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/approve` | `agent-access-approve-success` | `agent-access-approve-forbidden` | mongo-service-access |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/deny` | `agent-access-deny-success` | `agent-access-deny-forbidden` | mongo-service-access |
| `POST /api/v1/agents/{id}/approvals/{request_id}` | `agent-runtime-approval` | `agent-approval-not-pending-denied` | probe |
| `POST /api/v1/agents/{id}/artifacts` | `artifact-owner-publish` | `artifact-revoked-publication` | contract |
| `POST /api/v1/agents/{id}/attachments` | `attachment-success` | `attachment-foreign` | probe |
| `POST /api/v1/agents/{id}/browser/control` | `browser-control-acquire` | `browser-control-post-foreign` | contract |
| `POST /api/v1/agents/{id}/deliverable-references` | `deliverables-owner-describe-document` | `deliverables-revoked-describe-document` | contract |
| `POST /api/v1/agents/{id}/files` | `filesystem-create-success` | `filesystem-write-foreign` | contract |
| `POST /api/v1/agents/{id}/input` | `agent-runtime-input` | `agent-input-unavailable-denied` | probe |
| `POST /api/v1/agents/{id}/interrupt` | `agent-runtime-interrupt` | `agent-interrupt-no-turn-denied` | probe |
| `POST /api/v1/agents/{id}/recover-worktree` | `agent-worktree-recovery-accepted` | `agent-worktree-recovery-foreign` | contract |
| `POST /api/v1/agents/{id}/share-links` | `typed-link-create` | `typed-link-create-not-owner` | contract |
| `POST /api/v1/agents/{id}/skills` | `agent-skills-install-accepted` | `agent-skills-install-foreign` | probe |
| `POST /api/v1/agents/{id}/steer` | `agent-runtime-steer` | `agent-steer-no-turn-denied` | probe |
| `POST /api/v1/agents/{id}/stop` | `agent-stop` | `agent-stop-denied` | probe |
| `POST /api/v1/agui/run` | `agui-run-stopped-owner` | `agui-run-foreign` | contract |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment` | `binding-media-upload` | `binding-media-upload-forbidden` | binding-contract |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/supplement` | `binding-media-supplement` | `binding-media-supplement-forbidden` | binding-contract |
| `POST /api/v1/conversations` | `responses-create-conversation` | `conversations-create-workspace-missing` | context-creation |
| `POST /api/v1/documents` | `document-create-success` | `document-create-forbidden` | own-creation |
| `POST /api/v1/documents/{id}/comments` | `document-comment-success` | `document-comment-foreign` | probe |
| `POST /api/v1/documents/{id}/share-links` | `document-link-success` | `document-link-foreign` | probe |
| `POST /api/v1/resource-shares/{kind}/{id}` | `share-create-success` | `share-create-not-found` | contract |
| `POST /api/v1/responses` | `responses-create-completes` | `responses-create-foreign` | contract |
| `POST /api/v1/responses/{response_id}/cancel` | `responses-cancel-refused` | `` | unsupported |
| `POST /api/v1/sandboxes` | `sandbox-create` | `sandbox-create-denied` | profile-creation |
| `POST /api/v1/sandboxes/{id}/always-on` | `auto-pause-owner-disable-unset` | `auto-pause-owner-forbidden` | probe |
| `POST /api/v1/sandboxes/{id}/commands` | `sandbox-command-owner-admission` | `sandbox-command-revoked` | contract |
| `POST /api/v1/sandboxes/{id}/pause` | `sandbox-pause` | `sandbox-pause-denied` | probe |
| `POST /api/v1/sandboxes/{id}/revive` | `sandbox-revive-accepted` | `sandbox-revive-not-paused-denied` | probe |
| `POST /api/v1/sandboxes/{id}/runtime-update` | `runtime-update-noop` | `runtime-update-foreign` | contract |
| `POST /api/v1/sandboxes/{id}/service-access` | `sandbox-service-access-grant` | `sandbox-service-access-forbidden` | probe |
| `POST /api/v1/sandboxes/{id}/workspaces` | `state-workspace-parent-paused` | `workspace-create-denied` | probe |
| `POST /api/v1/sandboxes/{id}/workspaces/inline` | `inline-workspace-owner` | `inline-workspace-forbidden` | probe |
| `POST /api/v1/usage/metrics/reports` | `metrics-0-create-ready` | `metrics-0-create-forbidden` | mongo-report |
| `POST /api/v1/workspaces/{id}/agents` | `agent-create` | `agent-create-denied` | probe |
| `POST /api/v1/workspaces/{id}/recover-prepare` | `workspace-prepare-recovery-accepted` | `workspace-prepare-recovery-foreign` | contract |
| `POST /api/v2/agents/{agent_id}/responses` | `responses-v2-alias-retry` | `responses-v2-alias-foreign` | contract |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/controls` | `response-controls-v2-cancel` | `response-controls-v2-foreign-submit` | contract |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt` | `interruption-v2-admit` | `interruption-v2-foreign` | contract |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | `queued-cancellation-v2-owner-cancel` | `queued-cancellation-v2-foreign-submit` | contract |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input` | `queued-input-v2-owner-edit` | `queued-input-v2-foreign-submit` | contract |
| `POST /api/v2/responses` | `responses-v2-create-retry` | `responses-v2-create-foreign` | contract |
| `PUT /api/v1/agents/{id}/browser` | `browser-put-success` | `browser-put-foreign` | contract |
| `PUT /api/v1/agents/{id}/messages/{message_id}/feedback` | `feedback-reasons-down` | `message-feedback-set-foreign` | contract |
| `PUT /api/v1/agents/{id}/messages/{message_id}/feedback/reasons` | `feedback-reasons-save` | `feedback-reasons-put-foreign` | contract |
| `PUT /api/v1/agents/{id}/service-access` | `agent-service-access-success` | `agent-service-access-forbidden` | mongo-service-access |
| `PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | `binding-conversation-put` | `binding-put-conversation-forbidden` | binding-contract |
| `PUT /api/v1/documents/{id}/shares/{subject}` | `document-grant-success` | `document-grant-foreign` | probe |
| `GET /api/v1/agents/{id}/history` | `raw_history_allows_owner_and_denies_shared_and_foreign_before_export` | Same test + 1,000 interleavings | Real exporter boundary |
| `POST /api/v1/agent-tools` (`update_document`) | `publishes_large_private_document_replays_and_updates_with_cas` | `tenant_isolation_runtime_document_ids_and_old_receipts_never_expand_authority` | Real document service + runtime-auth double |
