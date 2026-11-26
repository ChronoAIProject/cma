<!-- openapi-operations:start -->

### Account deletion

| Operation | Behavior |
|---|---|
| `GET /api/v1/account/deletion` | Read the authenticated owner's deletion status, including when new requests are disabled. |
| `POST /api/v1/account/deletion` | Request deletion with explicit confirmation and `Idempotency-Key`; return 202 with a durable receipt. Replays remain available when new requests are disabled. |

### Legal consent

The deployment operator publishes at most one current version of its terms and
privacy notice. Without one, both operations answer 404 `legal_terms_not_configured`.

| Operation | Behavior |
|---|---|
| `GET /api/v1/account/legal-consent` | Read the current terms version, its document URLs and when the verified human owner accepted that version (`null` until accepted). |
| `PUT /api/v1/account/legal-consent` | Accept the exact current `version`; a repeat keeps the original acceptance time, and any other version is refused with 409 `legal_terms_version_mismatch`. An owner without a recorded browser sign-in gets 409 `legal_consent_sign_in_required` and must sign in again. |
| `GET /api/v1/account/privacy-export` | Page through the verified personal owner's Agent export inventory when account export is enabled. |
| `GET /api/v1/account/privacy-export/agents/{id}/transcript` | Deliver a bounded, verified retained-conversation page with a SHA-256 manifest and source-bound continuation. |

### Event Worker Profiles

These owner-scoped resources compose three published CMA profile revisions into
one full, immutable gateway configuration. All operations require
`manage_own_profiles`. Creation uses `Idempotency-Key`; updates and retirement
compare `expected_revision`. Repeating an identical successful update recovers
that revision; conflicting edits return 409. Retirement preserves revisions and
existing Bot snapshots. The `profile` field contains the complete
`EventWorkerProfile`; `source` records the three CMA references and authoring text.

| Operation | Behavior |
|---|---|
| `GET /api/v1/event-worker-profiles` | List active owned profiles using `after` and `limit` (1–200). |
| `POST /api/v1/event-worker-profiles` | Create and pin the complete configuration; return 201. |
| `GET /api/v1/event-worker-profiles/{id}` | Read the current owned revision, including retirement. |
| `PUT /api/v1/event-worker-profiles/{id}` | Save a new revision with `expected_revision` and `source`. |
| `DELETE /api/v1/event-worker-profiles/{id}` | Retire with query `expected_revision`, preserving history. |
| `GET /api/v1/event-worker-profiles/{id}/revisions/{revision}` | Read an immutable owned historical revision. |


### Bots

Creation optionally accepts `sandbox_profile:{id,revision}` and
`workspace_profile:{id,revision}` alongside the required Agent `profile`.
Explicit environment profiles must be personally owned and published; an omitted
reference pins the corresponding platform default. Bot responses expose all three
pinned references. Alternatively, supply `event_worker_profile:{id,revision}`
and the three matching source references to pin a saved full worker configuration.
The Bot retains that snapshot even if the worker is later edited or retired.
Reusing a creation key with different environment references
returns 409, including changing an omitted default into an explicit reference.

| Operation | Behavior |
|---|---|
| `POST /api/v1/bots` | Create a durable owner Bot with an Idempotency-Key; 201 returns creation progress. |
| `GET /api/v1/bots` | List owned Bots with exclusive `after` and `limit` (1–200). |
| `GET /api/v1/bots/connections` | List the signed-in owner’s ready Gateway connections as `{connection_ref,label}`. |
| `GET /api/v1/bots/nyxid-bots` | Compatibility inventory projection; new clients use `/connections`. |
| `GET /api/v1/bots/{bot_id}` | Read owned Bot state, operation progress, channel endpoints and last synthetic test. |
| `POST /api/v1/bots/{bot_id}/test` | Run a synthetic provider turn; does not send an external chat message. |
| `POST /api/v1/bots/{bot_id}/disable` | 202 admits durable denial of all source events. |
| `POST /api/v1/bots/{bot_id}/enable` | 202 admits restoration of the original source admission policy. |
| `POST /api/v1/bots/{bot_id}/rotate-keys` | 202 admits rotation of the consumer Agent credential (and the route credential for legacy consumers), with recovery and cleanup. |
| `DELETE /api/v1/bots/{bot_id}` | 202 admits deletion; GET returns 404 after provider, channel, routes and keys are removed. |

### Resource access and share management

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/organizations` | - | - | 200 OrganizationChoicesView; 401/403/503 ProblemDocument |
| `GET /api/v1/resource-access/{kind}/{id}` | - | - | 200 EffectiveAccessView |
| `GET /api/v1/resource-shares/{kind}/{id}` | - | - | 200 ActiveShareList |
| `POST /api/v1/resource-shares/{kind}/{id}` | CreateShareBody; key | - | 200 ShareReceiptView |
| `GET /api/v1/resource-shares/{kind}/{id}/{share_id}` | - | - | 200 ResourceShareView |
| `PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}` | UpdateShareBody; key | - | 200 ShareReceiptView |
| `DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}` | -; key | expected_revision* | 200 ShareReceiptView |
| `POST /api/v1/resource-revocations` | RevocationBody; key | - | 202 RevocationStatusView |
| `GET /api/v1/resource-revocations/{operation_id}` | - | kind*, id* | 200 RevocationStatusView |
| `POST /api/v1/cmaeg/worker-credentials/classify` | WorkerCredentialDigest | Gateway service bearer only | 200 WorkerCredentialClassificationView; 400/401/403/503 ProblemDocument |
| `PUT /api/v1/cmaeg/bindings/{binding_id}` | PutBindingRequest | - | 200 BindingView; 400/401/403/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}` | - | - | 200 BindingView; 401/403/404/409/503 ProblemDocument |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}` | - | - | 204 empty; 401/403/409/503 ProblemDocument |
| `POST /api/v1/cmaeg/bindings/{binding_id}/preparation-inspections` | InspectPreparationRequest | - | 200 PreparationInspectionView; 400/401/403/404/409/503 ProblemDocument |
| `POST /api/v1/cmaeg/bindings/{binding_id}/preparations` | PrepareBindingRequest | Idempotency-Key | 200 PreparationReceiptView; 400/401/403/404/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations` | - | - | 200 BindingConversationCatalogView; 401/403/404/409/503 ProblemDocument |
| `PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | optional empty object | - | 200/201 BindingConversationView; 400/401/403/404/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | - | - | 200 BindingConversationInspectionView; 400/401/403/404/409/503 ProblemDocument |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | - | - | 204 empty; 400/401/403/404/409/503 ProblemDocument |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets` | ResetConversationRequest; Idempotency-Key | - | 200 ConversationResetView; 400/401/403/404/409/503 ProblemDocument |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-drift` | DesiredProfileRequest | - | 200 DesiredProfileDriftView or null; 401/403/404/409/503 ProblemDocument |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-transfers` | TransferConversationRequest | Idempotency-Key | 200 ProfileTransferView; 400/401/403/404/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}` | - | - | 200 EventContext; 401/403/404/503 Responses error |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment` | raw binary; upload query | Content-Type, Content-Length | 201 AgentAttachment; 400/403/409/413/429/503 media error |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/attachment` | - | - | 200 AgentAttachment; 403/404/503 media error |
| `POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}/media/{attachment_index}/supplement` | BindingMediaSupplement | - | 202 supplementary command; 400/403/409/503 media error |
| `GET /api/v1/gateway/channel-records` | - | owner, after, limit | 200 GatewayRecordPageView |
| `PUT /api/v1/gateway/channel-records/{record_id}` | PutGatewayRecordRequest | - | 200 PutGatewayRecordView |
| `GET /api/v1/gateway/channel-records/{record_id}` | - | - | 200 GatewayRecordView; 404 ProblemDocument |
| `DELETE /api/v1/gateway/channel-records/{record_id}` | - | expected_version | 204 empty; 404/409 ProblemDocument |
| `GET /api/v1/gateway/connection-records` | - | owner, after, limit | 200 GatewayRecordPageView |
| `PUT /api/v1/gateway/connection-records/{record_id}` | PutGatewayRecordRequest | - | 200 PutGatewayRecordView |
| `GET /api/v1/gateway/connection-records/{record_id}` | - | - | 200 GatewayRecordView; 404 ProblemDocument |
| `DELETE /api/v1/gateway/connection-records/{record_id}` | - | expected_version | 204 empty; 404/409 ProblemDocument |
| `PUT /api/v1/gateway/interaction-records/{record_id}` | PutInteraction | - | 200 PutGatewayRecordView; 400/403/409/413/503 ProblemDocument |
| `GET /api/v1/gateway/interaction-records/{record_id}` | - | - | 200 GatewayRecordView; 404 empty; 400/403/503 ProblemDocument |
| `DELETE /api/v1/gateway/interaction-records/{record_id}` | - | expected_version | 204 empty; 404 empty; 400/403/409/503 ProblemDocument |
| `POST /api/v1/agent-delegations/identity` | AgentDelegationIdentityRequest | - | 200 identity or login URL; 400/401/403/404/409/503 problem |
| `POST /api/v1/agent-delegations/operations` | AgentDelegationOperationRequest | - | 200 result envelope; 400/401/403/404/409/503 problem |
| `POST /api/v1/bot-setups/actions` | AgentSetupAction | - | 200 SetupView; 400/403/404/409/503 ProblemDocument |
| `POST /api/v1/bot-setups/advance` | AdvanceSetup (compatibility) | - | 200 SetupView; 400/403/404/409/503 ProblemDocument |

### Administration

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/admin/activity` | - | actor, action, limit, cursor | 200 ActivityPageView |
| `GET /api/v1/admin/activity/actors` | ReadActivityLog and ReadUserDirectory | q (literal display name or subject) | 200 array<ActivityActorView> (up to 20 matches), 400/401/403/503 ProblemDocument |
| `GET /api/v1/admin/profiles` | - | kind, visibility, owner, cursor, limit | 200 ProfilePageView |
| `POST /api/v1/admin/profiles/{kind}/{id}/retire` | - | - | 200 ProfileSummaryView |
| `GET /api/v1/admin/runtime-inventory` | - | - | 200 RuntimeInventoryView |
| `GET /api/v1/admin/runtime-binding-holds` | - | - | 200 BindingHoldOverviewView; 403/503 ProblemDocument; 404 when diagnostics are disabled |
| `PUT /api/v1/admin/runtime-canary-policy` | CanaryPolicyRequest | - | 200 CanaryPolicyView; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/admin/runtime-canary-grants` | CanaryGrantRequest | - | 201 CanaryGrantView; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/admin/runtime-canary-grants/revoke` | CanaryRevokeRequest | - | 200 CanaryGrantView; 401/403/409/422/503 ProblemDocument |
| `GET /api/v1/admin/sandboxes` | - | owner, status, runtime_version, cursor, limit | 200 SandboxPageView |
| `GET /api/v1/admin/sandboxes/{id}` | - | - | 200 SandboxDetailView |
| `POST /api/v1/admin/sandboxes/{id}/always-on` | AdminAlwaysOnRequest | - | 200 SandboxView |
| `GET /api/v1/admin/sandboxes/{id}/events` | - | after, limit | 200 array<object> |
| `POST /api/v1/admin/sandboxes/{id}/retire` | - | - | 200 object |
| `POST /api/v1/admin/sandboxes/{id}/runtime-migration` | CustomRuntimeMigrationRequest | - | 202 SandboxView |
| `POST /api/v1/admin/sandboxes/{id}/runtime-update` | UpdateSandboxRuntimeRequest | - | 202 SandboxView |
| `GET /api/v1/admin/sandboxes/{id}/runtime-update/activity` | - | - | 200 object |
| `GET /api/v1/admin/admission` | - | - | 200 AdmissionReadView |
| `POST /api/v1/admin/admission/stop` | AdmissionChangeRequest | - | 200 AdmissionView |
| `PUT /api/v1/admin/admission/ceilings` | ReplaceCeilingsRequest | - | 200 CeilingsStateView |
| `POST /api/v1/admin/admission/pending-homes/reconcile` | ReconcilePendingHomesRequest | - | 200 PendingHomesReconciliationView |
| `POST /api/v1/admin/admission/resume` | AdmissionChangeRequest | - | 200 AdmissionView |
| `GET /api/v1/admin/settings` | - | - | 200 SettingsView |
| `GET /api/v1/admin/spend` | ConfigurePlatform; no write gate | - | 200 SpendView; 401 not_authenticated; 403 forbidden (not-an-operator); 503 platform_spend_unavailable. An absent service precedes the capability check. |
| `POST /api/v1/admin/spend/observations` | ConfigurePlatform; CHRONO_MODEL_BUDGET_WRITES_ENABLED and CHRONO_PLATFORM_SPEND_WRITES_ENABLED; ObservationRequest | - | 200 SpendView; 401 not_authenticated; 403 forbidden (not-an-operator); 422 invalid_request; 503 model_budget_writes_unavailable or platform_spend_unavailable. Identical observations at the same instant are idempotent; conflicting units return 422. |
| `PUT /api/v1/admin/settings` | ReplaceRequest | - | 200 SettingsView |
| `PATCH /api/v1/admin/settings` | PatchRequest | - | 200 SettingsView |
| `POST /api/v1/admin/settings/reset` | ResetRequest | - | 200 SettingsView |
| `GET /api/v1/admin/auto-pause/preview` | - | - | 200 AutoPausePreviewView |
| `POST /api/v1/admin/auto-pause/pause-idle-now` | PauseIdleNowRequest | - | 200 PauseIdleNowView |
| `POST /api/v1/admin/triggers/pause` | - | - | 200 SettingsView |
| `POST /api/v1/admin/triggers/resume` | - | - | 200 SettingsView |
| `GET /api/v1/admin/warm-pools` | - | - | 200 WarmPoolList |
| `POST /api/v1/admin/warm-pools` | ConfigureWarmPoolBody; key | - | 202 WarmPoolView |
| `PUT /api/v1/admin/warm-pools/{id}` | ResizeWarmPoolBody | - | 202 WarmPoolView |

All admission endpoints require `ConfigurePlatform`. `AdmissionView` is
`{revision, operator_stop, last_change, writes_available}`; GET returns
`AdmissionReadView`, which also includes `history`: at most 20 immutable
`AdmissionChange` records, newest first. Each change retains revision, Unix
`at`, actor, reason, `old_stop` and `new_stop`. Control, activity and immutable
history are written in one transaction; no-op requests add no records.
The control revision is independent of platform settings. Stop/resume bodies are
`{reason, expected_revision}`. Reasons contain 1–512 UTF-8 bytes, nonblank and
without control characters. Already stopped returns the first writer's current
stop, even for an old expected revision. Resume must name the current stop's
revision; stale writes return 409 `admission_control_conflict`. A closed write
gate returns 503 `admission_control_writes_unavailable`; reading and enforcing
stored control always remain active.

Fresh admissions while stopped return 503 `admission_paused` with
`Retry-After: 60` and the fixed public guidance, "New work is temporarily paused.
Existing work continues." Exact accepted replays and admitted continuations
proceed. The existing spend checks retain their refusal behavior. New subject
registration keeps 503 `registration_closed`, and signup options report
`register_available: false`; existing owners can sign in. See the complete
[admission contract](CMA_Admission_Control.md) and
[operator runbook](CMA_Operations.md#operator-admission-stop).

Warm pool views include `config_sha256`, the immutable image/resources/home/Pod
policy digest. Inventory includes `default_config_sha256` for the current default
configuration (`null` when planning is unavailable). Select the deployment default
by exact digest equality: old and new policies can share image and CPU/memory.
A new policy requires a new creation key; replay and resize retain the original
policy. New native pools use `fsGroupChangePolicy: OnRootMismatch`; historical
pools and retained homes keep their admitted templates.

### Agent profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agent-profiles` | - | cursor, limit | 200 AgentProfilePageView |
| `POST /api/v1/agent-profiles` | CreateAgentProfileRequest; key | - | 201 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}` | - | - | 200 AgentProfileView |
| `PUT /api/v1/agent-profiles/{id}` | UpdateAgentProfileRequest; key | - | 200 AgentProfileView |
| `POST /api/v1/agent-profiles/{id}/retire` | -; key | - | 200 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}/revisions` | - | cursor, limit | 200 AgentProfileRevisionPageView |
| `POST /api/v1/agent-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 AgentProfileRevisionView |
| `GET /api/v1/agent-profiles/{id}/revisions/{revision}` | - | - | 200 AgentProfileRevisionView |
| `PUT /api/v1/agent-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 AgentProfileView |

### Agents

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/desktop-capability` | - | workspace_id, sandbox_id | 200 DesktopCapabilityView |
| `POST /api/v1/agents` | AgentLifecycleCreateRequest; key | - | 202 AgentLifecycleCreateReceipt |
| `GET /api/v1/agent-lifecycle/{id}` | - | - | 200 AgentLifecycleOperationView |
| `POST /api/v1/agent-lifecycle/{id}/cancel` | - | - | 200 AgentLifecycleOperationView |
| `POST /api/v1/runtime-canary-redemptions` | CanaryRedeemRequest; Idempotency-Key | - | 202 AgentLifecycleCreateReceipt; 400/401/403/409/422/503 ProblemDocument |
| `GET /api/v1/agents` | - | organization, sandbox, workspace, status, cursor, limit | 200 AccessibleAgentPageView |
| `GET /api/v1/agents/{id}` | - | - | 200 AccessibleAgentView |
| `GET /api/v1/agents/{id}/capacity-recovery/dry-run` | - | - | 200 authorized recovery metadata |
| `GET /api/v1/agents/{id}/capacity-recovery` | - | - | 200 authorized recovery status |
| `POST /api/v1/agents/{id}/capacity-recovery/status` | Original recovery request | - | 200 original recovery status |
| `POST /api/v1/agents/{id}/capacity-recovery/admit` | Original recovery request | - | 200 recorded result; 202 unknown commit |
| `POST /api/v1/agents/{id}/capacity-recovery/resume` | Original recovery request | - | 200 recorded result; 202 unknown commit |
| `DELETE /api/v1/agents/{id}` | -; key | - | 202 empty |
| `GET /api/v1/agents/{id}/events` | - | after, limit | 200 AgentEventPageView |
| `GET /api/v1/agents/{id}/suggestions` | - | turn_sequence, answer_sequence | 200 SuggestionReadingView |
| `GET /api/v1/agents/{id}/message-feedback` | - | after, limit | 200 MessageFeedbackPageView |
| `GET /api/v1/agents/{id}/messages/{message_id}/feedback` | - | - | 200 FeedbackDetailsView |
| `PUT /api/v1/agents/{id}/messages/{message_id}/feedback/reasons` | SetFeedbackReasonsRequest | - | 200 FeedbackDetailsView; 409 stale version; 422 invalid reasons |
| `PUT /api/v1/agents/{id}/messages/{message_id}/feedback` | SetMessageFeedbackRequest | - | 200 MessageFeedbackView |
| `GET /api/v1/agents/provenance` | `shared`, `organization`, `sandbox`, `workspace`, `status`, `task_status`, `creation_origin`, `cursor`, `limit` | - | 200 AgentProvenanceInventoryPageView |
| `GET /api/v1/agents/{id}/provenance` | - | - | 200 AgentProvenanceView |
| `PUT /api/v1/agents/{id}/service-access` | ReplaceAgentServiceAccess; key | - | 202 AgentAccessRequestView |
| `POST /api/v1/agents/{id}/attachments` | Raw binary bytes; key | name* | 201 AgentAttachmentView |
| `GET /api/v1/agents/{id}/access-requests` | - | - | 200 AgentAccessRequestList |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/approve` | ApproveAgentAccessRequest; key | - | 200 AgentAccessRequestView |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/deny` | DenyAgentAccessRequest; key | - | 200 AgentAccessRequestView |
| `GET /api/v1/agents/{id}/settings` | - | - | 200 AgentSettingsView |
| `PATCH /api/v1/agents/{id}/settings` | UpdateAgentSettingsRequest; key | - | 200 AgentSettingsView |
| `GET /api/v1/agents/{id}/models` | - | - | 200 AgentModelsView |
| `GET /api/v1/agents/{id}/skills` | - | - | 200 AgentSkillsView |
| `POST /api/v1/agents/{id}/skills` | InstallAgentSkillRequest; key | - | 202 AgentSkillsView |
| `GET /api/v1/agents/{id}/source` | - | - | 200 AccessibleAgentSourceView |
| `POST /api/v1/agents/{id}/stop` | StopAgentRequest; key | - | 202 empty |
| `POST /api/v1/agents/{id}/restart` | Required key | - | 202 AgentRestartReceipt |
| `POST /api/v1/agents/{id}/recover-worktree` | -; required key | - | 202 AgentView |
| `GET /api/v1/agents/{id}/terminal` | - | - | 404 ProblemDocument; 501 ProblemDocument |
| `GET /api/v1/agents/{id}/desktop` | - | - | 200 DesktopDescriptor; 404 ProblemDocument |
| `GET /api/v1/agents/{id}/desktop/ws` | WebSocket upgrade; browser Origin | - | 101 RFB stream; 400/403 refusal; 404/409/503 ProblemDocument |
| `GET /api/v1/agents/{id}/transcript` | - | - | 200 AgentTranscriptView |
| `GET /api/v1/agents/{id}/retained-output` | - | cursor | 200 RetainedOutputPageView |
| `POST /api/v1/workspaces/{id}/agents` | CreateAgentRequest; key | - | 201 AgentView |

`recover-worktree` admits another initial worktree preparation attempt for a
failed Agent that has never become ready. The Agent ID and persistent volume are
preserved; the failed worktree and its evidence remain retained while a fresh
worktree is allocated. This bodyless operation requires the owner with CMA use
authority or a CMA administrator. Reuse the original key after an uncertain
response. HTTP 202 means recovery is admitted, not that preparation is complete.
Ineligible states return HTTP 409 `agent_worktree_recovery_conflict`.

### Agent home files

Paths are relative to the runtime's persistent `/home/` directory. Reading
requires current file-view authority; creating, editing and deleting require
owner authority or an accepted, active typed `view_write` sharing link. Each
operation rechecks access, and protected credential paths remain unavailable.
The mounted home intentionally includes shared Workspace source and Worktree
files; the Agent route is not a per-Agent filesystem grant. Public file operations
exclude supervisor control roots `.cma` and `cma/.cma`, including every private
Agent home and its instructions, credentials, sessions and retained state.
Those roots are omitted from listings and refused for direct reads, downloads,
writes, directory creation and deletion, including deletion of their ancestors.
The runtime also protects its configured state and checkpoint roots if they lie
within the mounted home. Descriptor-relative no-follow and single-link checks
continue to reject symlink/hardlink aliases.

Uploads live at `/home/attachments/{agent_id}/...`. A file route exposes only its
authorized Agent's upload namespace, including to the Sandbox owner; listing
`attachments` filters other Agents, and deleting that common root is refused.
Authorized ordinary Sandbox files, hidden workspace files, versioned edits,
downloads and typed shared writers retain their existing behavior. Trusted
runtime settings/recovery operations still access the selected Agent's managed
home through their separate internal contract; public file routes cannot do so.
Reads, downloads and writes are bounded to 32 MiB. A write's `expected_sha256`
protects an edit against intervening changes; omit it when creating a new file.
Directory creation uses `directory: true`. File operations do not require an
`Idempotency-Key`.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/environment` | - | - | 200 EnvironmentView |
| `GET /api/v1/agents/{id}/browser` | - | runtime_incarnation*, observer, context_id | 200 BrowserView |
| `PUT /api/v1/agents/{id}/browser` | BrowserEnablement | - | 200 BrowserView |
| `DELETE /api/v1/agents/{id}/browser` | - | runtime_incarnation*, observer* | 200 BrowserView |
| `GET /api/v1/agents/{id}/browser/control` | - | runtime_incarnation*, context_id*, client_id*, operation_id | 200 ControlView |
| `POST /api/v1/agents/{id}/browser/control` | ControlChange | - | 200 ControlView |
| `GET /api/v1/agents/{id}/files/tree` | - | path | 200 FileListView |
| `GET /api/v1/agents/{id}/files/content` | - | path*, max_bytes | 200 FileReadView |
| `GET /api/v1/agents/{id}/files/download` | - | path* | 200 application/octet-stream |
| `POST /api/v1/agents/{id}/files` | WriteFileRequest | - | 200 FileMutationView |
| `DELETE /api/v1/agents/{id}/files` | - | path*, recursive | 200 FileMutationView |

`GET /api/v1/agents/{id}/environment` returns a read-only capability observation
under current `view_details` authority. It contains `agent_id`, `policy_revision`,
an opaque nullable `runtime_incarnation`, nullable `revalidate_at`, and the
`files`, `operation_output`, `browser`, `replay`, and `control` features. Each
feature has `state`, `reason`, `scope`, and `writable`. States are `available`,
`unsupported`, `not_enabled`, `forbidden`, and `temporarily_unavailable`.
The observation is not a retained grant; every operation still authorizes its
caller. Responses use the existing private/no-store file-route policy.

Files retain the existing `mounted_home` scope, including `/home/`; this is not
an Agent-private directory. `writable` requires owner authority or an accepted
current typed write link. Recorded output has `agent_turn_history` scope and
requires `view_history`. Browser observations use `agent_browser` scope and
require `view_history`; `writable` remains false. Older runtimes without the
browser protocol report unsupported. Operation record replay uses `view_history`
and `agent_turn_history` scope, with `available`, `operation_records_only` and
`writable: false`; missing authority reports `forbidden`. Replay reuses bounded
Agent events v2 snapshots with `Prefer: cma-tool-output`, durable turn sequences
and opaque cursors. It is a pure history query without execution or recovery;
it contains no historical browser frames. Control reports `available`,
`exclusive_browser_control`, `agent_browser` and `writable: true` only for a
current owner with an enabled browser and verified runtime arbitration.
Unsupported, not enabled, forbidden and unknown states remain explicit. No
runtime address, boot identifier or PTY is exposed.

Browser control uses `GET /api/v1/agents/{id}/browser/control` for current
status/audit or a selected receipt and `POST` on the same route for mutations.
Both pin `runtime_incarnation`, `context_id` and `client_id`. GET optionally
selects an original `operation_id`. POST adds `command` (`acquire`, `input`,
`renew`, `return`), UUID `operation_id`, `expected_revision`, and `lease_token`
except for acquisition. Input additionally contains `{kind:"click",x,y}` within
1280×720, `{kind:"text",text}` up to 1024 UTF-16 units, or a supported editing
key without modifiers. Arbitrary scripts, shortcuts and desktop commands are
not part of this vocabulary. POST bodies are bounded to 16 KiB.

Mutations and selected receipt reads require current owner `manage_sharing`
authority and an independent private UUID in `X-CMA-Browser-Client`; Core binds
that key to the current account/credential. Public client/operation identifiers
do not grant lease recovery. Plain status/audit reads require `view_history` and
never return a token. Successful receipts include `state` (`agent`, `user`,
`unknown`), current `revision`, optional holder/expiry/token, original
`operation_id` and `operation_revision`, fixed `lease_duration_ms:15000`, and at
most 128 metadata-only audit entries with an explicit truncation boundary.
Neither audit nor URLs contain entered text, client keys or lease tokens.

User and native Agent operations share one runtime queue. A successful acquire
fences native input/navigation; return preserves the page and context and admits
native input again. Only explicit renewal extends the monotonic 15-second lease.
Lost responses require querying the original operation or retrying the identical
request, including revision, key and token. Reusing an operation for different
content, expired tokens, competing clients and old incarnations fail closed.
Revoked/offline clients cannot renew; Agent resumption occurs after the last
lease expires and in-flight input settles. Browser restart replaces the context.

Typed errors include `invalid_browser_control` (400), owner/access errors
(401/403/404), `browser_control_unsupported`, `browser_control_not_ready` and
`browser_control_conflict` (409), and `browser_control_unknown` (503). An unknown
outcome authorizes no further input. Audit and receipts are volatile per browser
context, not archival replay. Reader floors and rollback constraints are in
`docs/plans/cma-ui-optimization/17-CONTROL-CONTRACT.md`.

Browser enablement is explicit and owner-only (`manage_sharing` authority):
`PUT /api/v1/agents/{id}/browser` accepts
`{"runtime_incarnation":"<environment observation>","enabled":true}`. False
closes this browser. No browser starts at Sandbox startup, Agent startup or
environment read. The running Agent must already exist; at most one Agent
browser is enabled per Sandbox. Enablement belongs to this runtime incarnation
and is not persisted or automatically replayed after restart. It does not
restart the Agent, create a task, or enable user control. Ticket 16 owns any
Home creation integration. The runtime's private `cma-browser navigate URL`
command operates this same Agent context via its injected Unix socket; the
public API accepts no navigation, script, pointer or keyboard operation.

GET without `observer` reads status without capturing. GET with an observer
identifier (1–64 ASCII letters, digits or hyphens) requests a bounded real frame.
The service scopes that identifier to the caller and credential identity,
authorizes before reading, and revalidates policy and runtime after reading.
`runtime_incarnation` is mandatory; stale runtime identity returns 409. After
the first frame, clients send its opaque UUID `context_id` to fence later reads.
An empty paused, disabled or unsupported observation may withdraw that context;
a different live context cannot replace it. Current unauthorized reads return
403 or undisclosing 404. These fields are correlation values, never access grants.

The live Agent browser observation optionally includes `address`, the current
HTTP(S) origin or `about:blank`. It contains no credentials, path, query or
fragment. This additive field accompanies the existing current-frame response;
it is never a historical screen record. Older runtimes omit it and remain
readable without replacement. The web address bar uses this reported origin and
retains the existing location label when an origin is absent.

`BrowserView` contains Agent/incarnation/context, `state`, a sanitized `location`
(`Blank page`, `Local page` or `Web page`), nullable `frame`, and bounded counters.
It exposes no CDP address, Unix socket, profile path, URL query or credentials.
Frame content is the authorized page image, not a recording or historical replay.
States distinguish `not_enabled`, `unsupported`, `loading`, `ready` (enabled,
without a delivered frame), `live`, `paused`, and `disconnected`. A sandbox
startup failure reports unsupported; it never substitutes a fixture image.
The existing production namespace/security policy must permit Chromium's own
sandbox. The ticket 15 evidence records the current Linux policy blocker.

Each JPEG is 1280×720, at most 262144 decoded bytes, with a strictly increasing
sequence and actual `captured_at`. Captures are at least 1000 ms apart, including
across navigation; no more than 16 image deliveries occur in a rolling second.
There is one cached frame, 16 observer leases of 4000 ms, 32 queued mediator
operations, and a 360000-byte SDK response limit. Existing public file-route
capacity limits apply. `captures` counts real capture attempts returning image
data; `bytes` counts accepted decoded capture bytes, not wire bandwidth. There
is no capture timer when nobody requests observation. Page activity can continue
while observation is hidden or closed; browser CPU/memory do not necessarily
fall to zero. Disablement explicitly ends that page.

DELETE releases only the caller's observer lease; it preserves the page and
context. Web polls at one second while visible, aborts after four seconds, backs
off failures at 2/4/8 seconds, and releases on hiding, closing or scope changes.
Only a decoded frame less than 3000 ms old can display Live. Duplicate, delayed
or older-sequence frames cannot renew freshness or roll the image backward.
Transient failure retains a labeled last frame; permission/runtime loss removes
it. The TypeScript `agentBrowser` and Rust `agent_browser()` APIs implement the
same bounded read/configure/release contract. No request is automatically retried
as an enablement mutation.

Agent event reads opt into recorded output with `tool_output=true`. A tool update
may include `output: {text, truncated}`; `tool_output_reported` contains `call_id`
and `output` without a status. Text is bounded to 16 KiB of valid UTF-8 per
record, with explicit truncation. Absence means output was not reported. Legacy
requests omit output and retain the event cursor through redacted output-only
entries. See [Agent events v2](API_Agent_Events_V2.md) for negotiated snapshots
and tails, and [AG-UI](API_AGUI.md) for the stream projection.

### Agent sharing links

Only the Agent owner manages links and searches the recipient directory.
Redemption requires an authenticated recipient, the link token, and membership
in the configured audience. Public links still require authentication. Link
creation accepts a 32-byte cryptographically random token encoded as 64
lowercase hexadecimal characters; only its SHA-256 digest is retained. Keep the
token in the browser URL fragment and send it in the redemption JSON body.
Revocation is independent for each link and uses `expected_revision`. Links
grant Agent access without granting Sandbox inventory or lifecycle rights.
See [Agent sharing](Agent_Sharing.md) for audience and permission semantics.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/share-links` | - | - | 200 ShareLinkList |
| `POST /api/v1/agents/{id}/share-links` | CreateLinkBody; key | - | 200 ShareLinkView |
| `DELETE /api/v1/agents/{id}/share-links/{share_id}` | -; key | expected_revision* | 200 ShareLinkView |
| `GET /api/v1/agents/{id}/share-recipients` | - | q* | 200 RecipientList |
| `POST /api/v1/agent-share-links/{share_id}/redeem` | RedeemLinkBody | - | 200 ShareLinkView |

### Agent operations

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/agents/{id}/approvals/{request_id}` | ApprovalDecisionRequest; key | - | 202 empty |
| `POST /api/v1/agents/{id}/input` | AgentTextRequest; key | - | 202 InputAcceptedView |
| `POST /api/v1/agents/{id}/interrupt` | -; key | - | 202 empty |
| `POST /api/v1/agents/{id}/steer` | AgentTextRequest; key | - | 202 empty |

### Sandbox allocation usage

All quantities measure allocated resource-time. CPU and RAM accrue while the
runtime is bound; disk accrues while the persistent home remains allocated,
including while paused. Existing Sandboxes report partial lifecycle coverage.
See [Sandbox usage](CMA_Sandbox_Usage.md) for units, authorization, retention,
billing integration and TypeScript examples. Responses are private and no-store.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/usage` | - | - | 200 UsageSummaryView |
| `GET /api/v1/usage/sandboxes` | - | cursor, limit | 200 UsagePageView |
| `GET /api/v1/usage/sandboxes/{id}` | - | - | 200 SandboxUsageView |
| `GET /api/v1/usage/sandboxes/{id}/events` | - | cursor, limit | 200 UsageEventsView |
| `GET /api/v1/admin/usage` | - | owner | 200 UsageSummaryView |
| `GET /api/v1/admin/usage/sandboxes` | - | owner, cursor, limit | 200 UsagePageView |

### Measured Sandbox usage

These reports expose observed CPU, working-set memory and home filesystem
usage with independent coverage and allocated period quantities. See
[measured usage semantics and rollout](CMA_Sandbox_Metrics.md). Owner routes
require own-usage authority; administrator routes require `ReadAllSandboxes`.
All report, row, chart and owner-search reads reauthorize current access.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/usage/metrics/view` | - | window, from, to, step_seconds, filters, refresh | 200/202 UsageMetricView |
| `POST /api/v1/usage/metrics/reports` | UsageMetricQuery | - | 200/202 UsageMetricReport |
| `GET /api/v1/usage/metrics/reports/{report}` | - | - | 200 UsageMetricReport |
| `GET /api/v1/usage/metrics/reports/{report}/sandboxes` | - | limit, cursor, sort, order | 200 UsageMetricPage |
| `GET /api/v1/usage/metrics/reports/{report}/series` | - | metric*, group_by, series_id, top | 200 UsageMetricSeries |
| `GET /api/v1/usage/metrics/reports/{report}/sandboxes/{sandbox}` | - | - | 200 UsageMetricRow |
| `GET /api/v1/admin/usage/metrics/view` | - | window, from, to, step_seconds, filters, refresh | 200/202 UsageMetricView |
| `POST /api/v1/admin/usage/metrics/reports` | UsageMetricQuery | - | 200/202 UsageMetricReport |
| `POST /api/v1/admin/usage/metrics/options` | UsageFilterOptionsQuery | - | 200 UsageFilterOptions |
| `GET /api/v1/admin/usage/metrics/reports/{report}` | - | - | 200 UsageMetricReport |
| `GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes` | - | limit, cursor, sort, order | 200 UsageMetricPage |
| `GET /api/v1/admin/usage/metrics/reports/{report}/series` | - | metric*, group_by, series_id, top | 200 UsageMetricSeries |
| `GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes/{sandbox}` | - | - | 200 UsageMetricRow |
| `GET /api/v1/admin/usage/metrics/reports/{report}/owners` | - | search, cursor, limit | 200 UsageMetricOwners |
| `GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations` | - | metric, cursor, limit | 200 UsageOwnerAllocations |

### Agent usage

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/resources` | - | - | 200 AgentResourcesView |
| `GET /api/v1/agents/{id}/usage` | - | - | 200 AgentUsageView |

### Ag-ui

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/agents/{id}/sandbox` | - | - | 200 JSON |
| `GET /api/v1/agui/capabilities` | - | - | 200 JSON |
| `GET /api/v1/agui/events` | - | thread_id*, starting_after, after, follow | 200 SSE |
| `POST /api/v1/agui/run` | RunRequest | - | 200 SSE |
| `POST /api/v1/agui/run/receipt` | RunRequest / required Idempotency-Key | - | 200 InputReceipt or null |

### Trigger launches

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/launches/{id}/events` | - | starting_after, after, limit, follow | 200 SSE |
| `GET /api/v1/launches` | - | trigger, state, cursor, limit | 200 LaunchPageView |
| `GET /api/v1/launches/{id}` | - | - | 200 LaunchView |
| `POST /api/v1/launches/{id}/cancel` | -; key | - | 200 LaunchView |
| `GET /api/v1/launches/{id}/events` | - | after, limit | 200 LaunchEventPageView |
| `GET /api/v1/launches/{id}/requirements` | - | - | 200 RequirementsSnapshotView |
| `POST /api/v1/launches/{id}/start` | StartLaunchRequest; key | - | 202 LaunchView |
| `POST /api/v1/triggers/public/{public_id}/launches` | CreateLaunchRequest; key | - | 201 LaunchView; 202 LaunchView |
| `POST /api/v2/triggers/public/{public_id}/launches` | Require canonical `X-CMA-Expected-Subject` matching the authenticated caller before operation or quota access. Preserve v1 key/body/receipts and verify `X-CMA-Verified-Subject` before accepting terminal results. |
| `POST /api/v1/triggers/public/{public_id}/entry-intent` | - | - | 200 TriggerEntryIntentView |

### Discovery

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/conformance` | - | - | 200 ConformanceDocument |
| `GET /auth/signup-options` | - | - | 200 SignupOptions |

### Responses

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/conversations` | CreateConversationRequest | - | 200 Conversation |
| `GET /api/v1/conversations/{id}` | - | - | 200 Conversation |
| `GET /api/v1/conversations/{id}/items` | - | after, limit | 200 ConversationItemList |
| `GET /api/v2/ui/transport` | - | - | 200 UiTransportView |
| `GET /api/v2/ui/task-experience` | - | - | 200 TaskExperienceView |
| `GET /api/v2/agents/{agent_id}/events/head` | - | - | 200 EventHead |
| `GET /api/v2/agents/{agent_id}/events/snapshot` | - | limit, page_cursor | 200 EventPage |
| `GET /api/v2/agents/{agent_id}/events` | - | after_cursor, follow / Last-Event-ID | 200 Agent event SSE |
| `GET /api/v2` | - | - | 200 JSON capabilities |
| `POST /api/v2/responses` | CreateV2ResponseRequest | - | 200 SSE |
| `POST /api/v2/agents/{agent_id}/responses` | CreateV2ResponseRequest | - | 200 SSE |
| `POST /api/v2/agents/{agent_id}/responses/receipt` | CreateV2ResponseRequest / required Idempotency-Key | - | 200 V2CommandStatus or null |
| `GET /api/v2/responses/{response_id}` | - | - | 200 V2ResponseObject |
| `GET /api/v2/responses/{response_id}/events` | - | starting_after | 200 SSE |
| `GET /api/v2/responses/{response_id}/output` | - | after | 200 JSON output page |
| `GET /api/v2/responses/{response_id}/retained-output` | - | cursor | 200 RetainedOutputPageView |
| `GET /api/v2/conversations/{conversation_id}` | - | - | 200 Conversation |
| `GET /api/v2/conversations/{conversation_id}/items` | - | after, before, limit, order | 200 JSON items |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/controls` | - | - | 200 V2ResponseControls |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/controls` | V2ResponseControlRequest / required Idempotency-Key | - | 202 V2ResponseControl |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt` | V2InterruptRequest / required Idempotency-Key | - | 202 V2Interruption; atomic cancellation and distinct replacement response |
| `GET /api/v2/agents/{agent_id}/queued-inputs` | - | after, limit (1–4; default 4) | 200 V2QueuedInputPage |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input` | V2QueuedInputRequest / required Idempotency-Key | - | 200 V2QueuedInput |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | - | - | 200 V2QueuedCancellationObservation |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | V2QueuedCancellationRequest / required Idempotency-Key | - | 200 V2QueuedCancellation |
| `GET /api/v2/commands/{command_id}` | - | - | 200 V2CommandStatus |
| `POST /api/v1/responses` | CreateResponseRequest | - | 200 SSE |
| `GET /api/v1/responses/{response_id}` | - | - | 200 ResponseObject |
| `DELETE /api/v1/responses/{response_id}` | - | - | 422 ProblemDocument |
| `POST /api/v1/responses/{response_id}/cancel` | - | - | 422 ProblemDocument |

### Profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/defaults` | - | - | 200 DefaultProfilesView |

### Profile authoring

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/profile-authoring/bounds` | - | - | 200 ProfileCapacityBoundsView |
| `GET /api/v1/profile-authoring/services` | - | - | 200 array<AuthoringServiceView> |
| `GET /api/v1/profile-authoring/skills` | - | q, limit, cursor | 200 AuthoringSkillPage |
| `POST /api/v1/profile-authoring/repositories/resolve` | ResolveRepositoryRequest | - | 200 RepositoryResolutionView |

### Profile marketplace

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/marketplace/{kind}` | - | q, cursor, limit | 200 MarketplacePageView |
| `GET /api/v1/marketplace/{kind}/{id}` | - | - | 200 MarketplaceEntryView |
| `POST /api/v1/marketplace/{kind}/{id}/fork` | ForkProfileRequest; key | - | 201 ForkedProfileView |

### Sandbox profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandbox-profiles` | - | cursor, limit | 200 SandboxProfilePageView |
| `POST /api/v1/sandbox-profiles` | CreateSandboxProfileRequest; key | - | 201 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}` | - | - | 200 SandboxProfileView |
| `PUT /api/v1/sandbox-profiles/{id}` | UpdateSandboxProfileRequest; key | - | 200 SandboxProfileView |
| `POST /api/v1/sandbox-profiles/{id}/retire` | -; key | - | 200 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}/revisions` | - | cursor, limit | 200 SandboxProfileRevisionPageView |
| `POST /api/v1/sandbox-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 SandboxProfileRevisionView |
| `GET /api/v1/sandbox-profiles/{id}/revisions/{revision}` | - | - | 200 SandboxProfileRevisionView |
| `PUT /api/v1/sandbox-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 SandboxProfileView |

### Agent channel usage

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/channel-usage` | - | - | 200 AgentChannelUsageView |
| `GET /api/v1/agents/{id}/channel-usage/history` | - | cursor, limit | 200 AgentChannelUsageHistoryView |
| `GET /api/v1/agents/{id}/provider-associations` | - | cursor, limit | 200 AgentProviderAssociationsView |

Channel usage is separate from immutable creation origin. An authorized summary
contains `agent_id` and `channel_usage` with `active`, `has_history`, nullable
`first_used_at` and nullable `last_used_at`. Only current bridge managers receive
`bindings` or can request paginated history. Binding attachment records history
without inventing an accepted external command. History retains actor, time,
reason and before/after binding generations, including reset, rebind and detach.
Current policy, original authority deadline and retained Agent identity are
rechecked after buffering. Public callers cannot write channel facts through
these routes, and channel use grants no additional CMA resource rights.


**Coverage:** `channel-usage` and its history describe the legacy explicit Bridge
usage projection only. Provider binding/conversation admission does not populate
that projection. An empty result does not establish that the Agent was never
associated with a provider, and must not be used as deletion eligibility. Use
`provider-associations` for retained Manager membership; neither route invents
missing event-use history.

The creation inventory also returns the safe `channel_usage` summary on each row.
`channel_active=true|false` and `channel_history=true|false` combine with creation
origin, status and resource scope before pagination. For example, a manual Agent
with `channel_active=false&channel_history=true` retains its manual creation
origin after detachment. Empty filtered pages can have a continuation cursor.
Private binding, actor and external conversation references are absent from
inventory rows and exports.

### Approved Agent artifacts

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/artifacts` | - | cursor, limit (1–50) | 200 AgentArtifactPageView |
| `POST /api/v1/agents/{id}/artifacts` | PublishAgentArtifactRequest; key | - | 201 AgentArtifactView, including idempotent replay |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}` | - | - | 200 AgentArtifactView |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}/download` | - | - | 200 application/octet-stream attachment |

Publication requires current `manage_sharing` authority and an explicit selected
`{name, media_type, content_base64}` body. No caller owner, actor, filesystem path
or remote URL is accepted. It records immutable bytes and publication audit
under exact retained Agent ancestry. Content must be 1 byte through 1 MiB of
canonical standard base64. Display names contain 1–160 UTF-8 bytes, trimmed,
without controls, slash/backslash or `.`/`..`. Supported types are `text/plain`,
`text/markdown`, `application/json`, `application/pdf`, `application/octet-stream`,
`image/png`, `image/jpeg` and `image/webp`. There are at most 256 artifacts per
Agent. Invalid content returns `invalid_artifact_request` (400); full capacity
returns `artifact_capacity` (429). Keep the same key/body when the result is
uncertain; a changed request under the same key returns `idempotency_key_reused`.

Metadata exposes exactly `id`, `agent_id`, `name`, `media_type`, `size_bytes`,
`sha256` and `created_at`. `view_artifacts` grants access to this explicit registry,
including for narrow Agent readers. It does not add `view_files` or reveal parent
paths/archives. Reads recheck current policy and the original authority after
buffering; revoked/expired access prevents later downloads. Download filenames
in HTTP headers are generated from the content hash, with `nosniff` and
`private, no-store`. SDK downloads return bytes; the web verifies size/hash and
current permission before creating its local file. Previously downloaded copies
cannot be recalled. Sandbox state is persisted by the attached Agent Sandbox
volume; CMA does not expose a Sandbox snapshot archive endpoint.

### Answer deliverables

| Operation | Body | Query | Response |
|---|---|---|---|
| `POST /api/v1/agents/{id}/deliverable-references` | Explicit resource selector | - | 200 DeliverableDescriptionView |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables` | - | - | 200 AnswerDeliverablesView |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}` | - | `verify` (default false) | 200 AnswerDeliverableReadingView |
| `GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download` | - | - | 200 opaque attachment; 409 changed bytes |
| `POST /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/confirmation` | TaskDeliveryConfirmation | - | 200 TaskAcceptanceView |

Selectors are `{kind:"agent_artifact",id}`, `{kind:"document",id,revision}` or
`{kind:"live_file",path}`. The descriptor validates the resource's own access and
actual version, returning its Agent, selector, safe name, media type, exact byte
count and lowercase SHA-256. It does not mutate, publish or grant sharing.
Published artifacts and document sources retain their existing 1 MiB limits;
live files retain the filesystem's 32 MiB limit and home-relative path rules.

The marker is `<!--cma-deliverable:v1:<base64url-without-padding JSON reference>-->`.
Only a standalone unindented marker outside code fences in the final recorded
nonempty Assistant message of a completed turn is eligible. The server derives
Agent, turn and `event:<sequence>` answer identity from that immutable log, rather
than trusting an answer identity supplied in a marker. At most 32 distinct
references survive stable deduplication; the same resource may be explicitly
referenced in another answer but is never implicitly inherited. Marker payloads
are at most 4096 encoded bytes. Ordinary file names and code spans are not parsed.

Lists return `{agent_id,answer_id,turn_sequence,items}`. Single-item reads return
the same identity fields with `item`. Each item has `{id,reference,status}`.
Status is `ready`, `unchecked`, `changed`, `version_changed`, `missing`, `denied`,
`not_ready`, `too_large` or `unavailable`. Lists use immutable metadata and current
resource authorization. A live file is `unchecked` until download or explicit
`verify=true` checks that selected file's bytes. The stored reference remains the
answer-time evidence even after a file changes. A newer document source reports
`version_changed` and downloads the original source revision when authorized.
Live-file verification and download request at most the reference's byte count
from the runtime (one byte for an empty reference, which must still return zero
bytes). Growth beyond that bound, a smaller file, or changed content returns
`changed` without expanding that read budget. Initial live-path description
retains the 32 MiB cap and its `too_large` refusal.

Each request requires current answer visibility and independent resource access.
Document token links retain `X-CMA-Document-Share-Token` plus authentication.
Reads/downloads use private,no-store, nosniff and no-referrer; downloads are always
opaque attachment bytes with a percent-encoded safe filename and SHA-256 ETag.
Clients must bound buffers, verify size/digest and recheck current authorization
before saving. A narrow conversation share never publishes a private file or
adds file/document permission. Previously downloaded bytes cannot be recalled.

Answer verification targets one committed turn, scanning at most 128 events
after the requested message and 512 events in that turn, with a 4 MiB/15 second
projection bound. Exceeded budgets return 503 without treating the Agent as
missing. Gaps or inconsistent boundaries fail closed. HTTP requests
have a 30 second deadline. No list or per-card authorization rebuilds the full
conversation, and listing does not download file bodies. See
[Agent Manager](API_Agent_Manager.md#files-settings-and-access) for client usage.

### Sandboxes

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes` | - | organization, shared, status, cursor, limit | 200 AccessibleSandboxPageView |
| `GET /api/v1/sandboxes/capacity` | Observe authenticated owner Sandbox quota. |
| `POST /api/v1/sandboxes` | CreateSandboxRequest; key | - | 201 SandboxView |
| `GET /api/v1/sandboxes/{id}` | - | - | 200 AccessibleSandboxDetailView |
| `DELETE /api/v1/sandboxes/{id}` | -; key | - | 202 SandboxView |
| `GET /api/v1/sandboxes/{id}/events` | - | after, limit | 200 array<SandboxEventView> |
| `POST /api/v1/sandboxes/{id}/pause` | -; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/commands` | SandboxCommandRequest; key | - | 202 SandboxCommandReceiptView |
| `GET /api/v1/sandbox-commands/{id}` | - | - | 200 SandboxCommandReceiptView |
| `POST /api/v1/sandboxes/{id}/revive` | -; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/always-on` | AlwaysOnRequest | - | 200 SandboxView |
| `POST /api/v1/sandboxes/{id}/runtime-update` | UpdateSandboxRuntimeRequest; key | - | 202 SandboxView |
| `GET /api/v1/sandboxes/{id}/runtime-recovery` | - | - | 200 RetainedRecoveryView |
| `POST /api/v1/sandboxes/{id}/runtime-recovery` | Idempotency-Key | RecoverRetainedRequest | 202 RetainedReopenView |
| `POST /api/v1/sandboxes/{id}/service-access` | GrantSandboxServiceAccessRequest; key | - | 200 SandboxView; new grants require fully Paused, existing grants replay without change |

The read-only `runtime-recovery` assessment returns a fixed `disposition` and
`admission_enabled`, which describes the deployment gate rather than eligibility.
GET never schedules recovery. The explicit owner POST repeats stronger native and
history proof and requires consent, the reviewed revision and an original request
key. See [Same-image retained recovery](CMA_Retained_Recovery.md) for supported
states, refusal categories, atomic admission and rollback qualification.

`runtime-update` accepts `{ "target_version": "<exact image reference>" }`
with `Idempotency-Key` from the Sandbox owner. Use the owner detail's
`runtime_update.latest_version`, or its retained `target_version` when retrying a
failed update. A failed update stalled at reopening conversations whose retained
target is no longer the latest is recovered onto the latest runtime instead.
HTTP 202 records the request; follow `runtime_update.state` on
Sandbox reads until completion. Busy Agents defer the update until idle, and
eligible managed Sandboxes retain their original home and conversations. Stale
or conflicting targets return 409; replay an uncertain request with its original
body and key.

Operators with `ControlAnySandbox` can queue the same update across owners using
`POST /api/v1/admin/sandboxes/{id}/runtime-update` with that request body. This
operator endpoint does not require an idempotency key: retrying the same pending
target leaves the request unchanged. It validates the configured latest target
or an already pinned retry target and audits the actual operator, Sandbox owner,
and admitted target. After a stalled reopen is recovered onto the latest runtime,
its former target returns 409. Queueing does not interrupt current work; the existing idle checks
and transaction fencing retain the original PVC and conversation identities.
Operator Sandbox detail exposes `runtime_update` with the pinned target, current
state, and completion or failure metadata. The owner endpoint still hides
other owners' Sandboxes, including from operators.

Sandbox inventory is personal by default. `organization=<canonical ID>` selects
current organization ownership and whole-Sandbox grants. `shared=true` selects the
deduplicated union of incoming whole-Sandbox grants across current organizations,
excluding the caller's own personal resources before pagination. The two selectors
are mutually exclusive. Narrow Agent grants never reveal parent rows or counts.
Every aggregate page requires complete current membership discovery and checks
policy, evidence lifetime and hidden lookahead after count reads. Dependency
failure refuses the result without partial rows or a stale continuation. Both
SDKs expose this scope through their accessible Sandbox list method.

### Workspaces

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes/{id}/workspaces` | - | - | 200 AccessibleWorkspaceListView |
| `POST /api/v1/sandboxes/{id}/workspaces/inline` | CreateInlineWorkspaceRequest; key | - | 201 WorkspaceView |
| `POST /api/v1/sandboxes/{id}/workspaces` | CreateWorkspaceRequest; key | - | 201 WorkspaceView |
| `GET /api/v1/workspaces/{id}` | - | - | 200 AccessibleWorkspaceDetailView |
| `DELETE /api/v1/workspaces/{id}` | -; key | - | 202 WorkspaceView |
| `POST /api/v1/workspaces/{id}/recover-prepare` | -; required key | - | 202 WorkspaceView |

`recover-prepare` admits another preparation attempt for the same failed Workspace;
HTTP 202 confirms admission, not readiness. The Workspace must never have reached
Ready, have no resolved commit, Agents, Worktrees, pending preparation or active
lease, and retain an Active, idle parent Sandbox. Its original identity, source,
path, profile and data remain bound to the recovery. The owner needs
`UseOwnSandboxes`; an operator with `ControlAnySandbox` may also admit recovery,
with the actual operator retained as the requester.

The command has no body and requires a valid `Idempotency-Key`. Repeating the same
key replays its result. A new key is eligible only after a previous attempt has
failed terminally; it cannot bypass active or uncertain work. Missing or invalid
keys return 400, unauthenticated callers 401, and insufficient capability 403.
Missing or foreign Workspaces return 404. Ineligible state returns 409 with
`workspace_prepare_recovery_conflict`; unavailable storage returns 503. Read the
original Workspace to observe eventual preparation success or failure.

### Service connections

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/service-connections` | CreateServiceConnectionRequest; key | - | 201 CreatedServiceConnectionView |
| `GET /api/v1/service-connections/{handoff_id}` | - | - | 200 ServiceConnectionStatusView |

### Triggers

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers` | - | cursor, limit | 200 TriggerPageView |
| `POST /api/v1/triggers` | CreateTriggerRequest; key | - | 201 TriggerView |
| `GET /api/v1/triggers/public/{public_id}` | - | - | 200 TriggerPreviewView |
| `GET /api/v1/triggers/public/{public_id}/badge.svg` | - | label, theme, placement | 200 SVG; 304 empty |
| `GET /api/v1/triggers/public/{public_id}/open/{placement_id}` | - | - | 303 public Trigger page |
| `GET /api/v1/triggers/{id}` | - | - | 200 TriggerView |
| `PUT /api/v1/triggers/{id}` | UpdateTriggerRequest; key | - | 200 TriggerView |
| `DELETE /api/v1/triggers/{id}` | -; key | - | 204 empty |
| `POST /api/v1/triggers/{id}/publish` | PublishTriggerRequest; key | - | 201 PublishedTriggerRevisionView |
| `POST /api/v1/triggers/{id}/retire` | -; key | - | 200 TriggerView |
| `GET /api/v1/triggers/{id}/revisions` | - | - | 200 array<TriggerRevisionView> |
| `GET /api/v1/triggers/{id}/stats` | - | - | 200 TriggerStatisticsView |

Trigger deletion removes unpublished drafts and archives previously published
Triggers. Archived records disappear from owner reads/listing and refuse new
Launches; already-admitted Launches retain their frozen revisions and resources.
The exact successful deletion command remains replayable with its original key.

### Trigger placements

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers/{id}/embed` | - | placement_id* | 200 EmbedDescriptor |
| `GET /api/v1/triggers/{id}/placements` | - | include_retired | 200 array<PlacementView> |
| `POST /api/v1/triggers/{id}/placements` | CreatePlacementRequest; key | - | 201 PlacementView |
| `POST /api/v1/triggers/{id}/placements/{placement_id}/retire` | -; key | - | 200 PlacementView |

### Workspace profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspace-profiles` | - | cursor, limit | 200 WorkspaceProfilePageView |
| `POST /api/v1/workspace-profiles` | CreateWorkspaceProfileRequest; key | - | 201 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}` | - | - | 200 WorkspaceProfileView |
| `PUT /api/v1/workspace-profiles/{id}` | UpdateWorkspaceProfileRequest; key | - | 200 WorkspaceProfileView |
| `POST /api/v1/workspace-profiles/{id}/retire` | -; key | - | 200 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}/revisions` | - | cursor, limit | 200 WorkspaceProfileRevisionPageView |
| `POST /api/v1/workspace-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 WorkspaceProfileRevisionView |
| `GET /api/v1/workspace-profiles/{id}/revisions/{revision}` | - | - | 200 WorkspaceProfileRevisionView |
| `PUT /api/v1/workspace-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 WorkspaceProfileView |

### Worktrees

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspaces/{id}/worktrees` | - | - | 200 AccessibleWorktreeListView |
| `GET /api/v1/worktrees/{id}` | - | - | 200 AccessibleWorktreeView |



### Personal document Artifacts

Persistent documents are independent of their originating Agent, Workspace and Sandbox. `/api/v1/documents` is an authenticated per-user registry backed by majority-durable Mongo metadata in `cma_document_artifacts`; immutable Markdown sources live in the configured S3-backed object service. Agent/Sandbox deletion does not delete documents. List/search reads bounded metadata projections and never reads source objects or history/receipt payloads.

All writes require `Idempotency-Key`. Every metadata change increments `revision`; only title/source edits advance `source_revision` and add an immutable source revision. Owner title/source edits, grant/link changes and comment removal carry `expected_revision`. Stale revisions return 409; retries with the original key replay without repeating effects; different bodies under the same key return 409. Create replay returns current document metadata. Comments use a policy-fenced CAS with bounded retry and may anchor a specific source revision. Owner can remove any comment; a currently authorized commenter can remove their own.

Owner may grant `read` or `comment` to explicit authenticated subjects, or create authenticated token links. The browser generates a CSPRNG 256-bit token (64 lowercase hex), retains it only in the URL fragment and submits it as `X-CMA-Document-Share-Token`; the backend retains only SHA-256. Authentication is required in addition to the token. Links do not make documents searchable. Revocation is checked again after bounded source download, before bytes are returned. Document responses use private,no-store; nosniff; no-referrer. Ordinary `origin_agent_id` is an informational label and grants no runtime publishing authority.

Runtime MCP `publish_document` and `update_document` derive owner/Agent from confirmed runtime capability; no arbitrary owner field is accepted. Runtime update can change only verified runtime-authored documents for the current Agent/owner. The private runtime author field is separate from caller-supplied origin labels. Full runtime/conversation identity is checked again immediately before metadata commit after object upload, and replay requires current capability. Documents remain owner-accessible even if the originating runtime is removed.

Limits: UTF8 Markdown 1 MiB, title 240 bytes, search 160 bytes, comments 8 KiB; page 1–50 (default 25); source revisions 256; direct grants 128; links 32; comments 256; retained receipts 1024; serialized metadata 12 MiB. Sources are content-addressed by server-derived owner/document/digest keys; callers never select storage locations. A failed metadata CAS may leave an unreachable immutable source object; no document metadata or source is lost. There is no document deletion endpoint in this increment.

Errors:400 invalid_document_request/missing_idempotency_key/invalid_idempotency_key/invalid_json;401 not_authenticated;404 document_not_found conceals unavailable authority;409 document_revision_conflict/idempotency_key_reused;413 invalid_json size;415 json_content_type_required;422 invalid_json shape;429 document_capacity;503 document_unavailable.

| Operation | Behavior |
|---|---|
| `GET /api/v1/documents` | List/search owned and explicitly shared documents. |
| `POST /api/v1/documents` | Create owned Markdown document. |
| `GET /api/v1/documents/{id}` | Read effective document metadata. |
| `PATCH /api/v1/documents/{id}` | Owner source/title compare-and-swap. |
| `GET /api/v1/documents/{id}/source` | Read current or specific source revision. |
| `GET /api/v1/documents/{id}/revisions` | Page immutable source revision metadata. |
| `GET /api/v1/documents/{id}/shares` | Owner direct recipient inventory. |
| `GET /api/v1/documents/{id}/share-recipients` | Owner search for eligible direct recipients. |
| `PUT /api/v1/documents/{id}/shares/{subject}` | Grant/update read or comment access. |
| `DELETE /api/v1/documents/{id}/shares/{subject}` | Revoke direct recipient access. |
| `GET /api/v1/documents/{id}/share-links` | Owner link metadata, never secrets. |
| `POST /api/v1/documents/{id}/share-links` | Register authenticated token link. |
| `DELETE /api/v1/documents/{id}/share-links/{link_id}` | Revoke authenticated token link. |
| `GET /api/v1/documents/{id}/comments` | Page document comments. |
| `POST /api/v1/documents/{id}/comments` | Add comment under current grant. |
| `DELETE /api/v1/documents/{id}/comments/{comment_id}` | Owner or author comment removal. |

SDK surfaces: TypeScript `client.documents`; Rust `client.documents()`. Both expose list/create/read/update/source/revisions/shares/grant/revoke/links/createLink(create_link)/revokeLink(revoke_link)/comments/comment/removeComment(remove_comment). Read/comment options accept an explicit share token only in the header. Full and focused AgentManager OpenAPI documents include all 15 operations.



| `GET /api/v1/agents/{id}/preferences` | - | - | 200 AgentPreferencesView |
| `PATCH /api/v1/agents/{id}/preferences` | UpdateAgentPreferencesRequest; key | - | 200 AgentPreferencesView |

| `POST /api/v1/telemetry/browser` | Anonymous, bounded browser diagnostic category; no durable receipt. |
| `POST /api/v1/usage/metrics/initial/reports` | UsageInitialCreate | - | 200 complete initial data / 202 descriptor only |
| `POST /api/v1/usage/metrics/initial/view` | UsageInitialViewInput | - | 200 complete initial data / 202 descriptor only |
| `POST /api/v1/usage/metrics/initial/reports/{report}` | UsageInitialProjection | - | 200 complete initial data / 202 descriptor only |
| `POST /api/v1/admin/usage/metrics/initial/reports` | UsageInitialCreate | - | 200 complete initial data / 202 descriptor only |
| `POST /api/v1/admin/usage/metrics/initial/view` | UsageInitialViewInput | - | 200 complete initial data / 202 descriptor only |
| `POST /api/v1/admin/usage/metrics/initial/reports/{report}` | UsageInitialProjection | - | 200 complete initial data / 202 descriptor only |
| `GET /api/v1/agents/{id}/original-request` | Read the current owner’s exact admitted Task request without scanning history. |
| `GET /api/v1/agents/{id}/reply-preview` | Read a bounded certified preview of the latest persisted assistant message. |
| `POST /api/v1/sandbox-creation-outcomes/lookup` | Read evidence for an exact owner Sandbox creation. |
| `PUT /api/v1/admin/retained-runtime-policy` | CanaryPolicyRequest | - | 200 CanaryPolicyView; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/admin/retained-runtime-preview` | RetainedRuntimeProposal | - | 200 RetainedRuntimePreview; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/admin/retained-runtime-grants` | RetainedRuntimeGrantRequest | - | 201 RetainedRuntimeGrantView; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/retained-runtime-redemptions` | RetainedRuntimeRedemption | - | 202 RetainedRuntimeReceipt; 401/403/409/422/503 ProblemDocument |
| `POST /api/v1/admin/retained-runtime-grants/abort` | RetainedRuntimeAbort | - | 200 RetainedRuntimeGrantView; 401/403/409/422/503 ProblemDocument |
<!-- openapi-operations:end -->
