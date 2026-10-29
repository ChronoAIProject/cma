use super::context::Case;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Local,
    TypedShape,
    MediaType,
    RawJson,
    BodySize,
    QueryShape,
    BinaryHeaders,
}

// Exact backend-invalid cases not claimed as typed HTTP replay. Each is checked
// against its source shape or the SDK's real preflight, then its shared error is decoded.
pub const LOCAL_CASES: &[(&str, Classification)] = &[
    (
        "binding-media-missing-length",
        Classification::BinaryHeaders,
    ),
    ("binding-media-missing-type", Classification::BinaryHeaders),
    ("binding-media-encoding", Classification::BinaryHeaders),
    ("binding-media-too-large", Classification::BinaryHeaders),
    (
        "binding-media-incomplete-body",
        Classification::BinaryHeaders,
    ),
    ("preparation-inspect-invalid-json", Classification::RawJson),
    ("preparation-reserve-invalid-json", Classification::RawJson),
    ("preparation-reserve-missing-key", Classification::Local),
    ("agent-worktree-recovery-missing-key", Classification::Local),
    ("agent-worktree-recovery-invalid-key", Classification::Local),
    (
        "workspace-prepare-recovery-missing-key",
        Classification::Local,
    ),
    (
        "workspace-prepare-recovery-invalid-key",
        Classification::Local,
    ),
    ("document-create-missing-key", Classification::Local),
    ("document-create-invalid-key", Classification::Local),
    ("document-create-malformed-json", Classification::RawJson),
    (
        "document-create-missing-media-type",
        Classification::MediaType,
    ),
    ("document-update-missing-key", Classification::Local),
    ("document-update-invalid-key", Classification::Local),
    ("document-update-malformed-json", Classification::RawJson),
    (
        "document-update-missing-media-type",
        Classification::MediaType,
    ),
    ("document-source-invalid", Classification::TypedShape),
    ("document-grant-missing-key", Classification::Local),
    ("document-grant-invalid-key", Classification::Local),
    ("document-grant-malformed-json", Classification::RawJson),
    (
        "document-grant-missing-media-type",
        Classification::MediaType,
    ),
    ("document-revoke-missing-key", Classification::Local),
    ("document-revoke-invalid-key", Classification::Local),
    ("document-revoke-malformed-json", Classification::RawJson),
    (
        "document-revoke-missing-media-type",
        Classification::MediaType,
    ),
    ("document-link-missing-key", Classification::Local),
    ("document-link-invalid-key", Classification::Local),
    ("document-link-malformed-json", Classification::RawJson),
    (
        "document-link-missing-media-type",
        Classification::MediaType,
    ),
    ("document-unlink-missing-key", Classification::Local),
    ("document-unlink-invalid-key", Classification::Local),
    ("document-unlink-malformed-json", Classification::RawJson),
    (
        "document-unlink-missing-media-type",
        Classification::MediaType,
    ),
    ("document-comment-missing-key", Classification::Local),
    ("document-comment-invalid-key", Classification::Local),
    ("document-comment-malformed-json", Classification::RawJson),
    (
        "document-comment-missing-media-type",
        Classification::MediaType,
    ),
    ("document-uncomment-missing-key", Classification::Local),
    ("document-uncomment-invalid-key", Classification::Local),
    ("document-uncomment-malformed-json", Classification::RawJson),
    (
        "document-uncomment-missing-media-type",
        Classification::MediaType,
    ),
    (
        "filesystem-tree-reject-unknown-query",
        Classification::QueryShape,
    ),
    (
        "filesystem-content-reject-unknown-query",
        Classification::QueryShape,
    ),
    (
        "filesystem-download-reject-unknown-query",
        Classification::QueryShape,
    ),
    (
        "filesystem-delete-reject-unknown-query",
        Classification::QueryShape,
    ),
    (
        "filesystem-delete-reject-invalid-recursive",
        Classification::TypedShape,
    ),
    (
        "filesystem-content-reject-invalid-max-bytes",
        Classification::TypedShape,
    ),
    (
        "filesystem-write-reject-unknown-field",
        Classification::TypedShape,
    ),
    (
        "filesystem-write-reject-malformed-json",
        Classification::RawJson,
    ),
    ("admin-runtime-update-invalid-json", Classification::RawJson),
    ("attachment-invalid-name", Classification::Local),
    ("attachment-missing-key", Classification::Local),
    ("agent-skills-install-invalid-id", Classification::Local),
    (
        "agent-skills-install-invalid-version",
        Classification::Local,
    ),
    ("agent-skills-install-missing-key", Classification::Local),
    ("agent-skills-install-invalid-key", Classification::Local),
    (
        "agent-skills-install-unknown-field",
        Classification::TypedShape,
    ),
    (
        "agent-skills-install-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-skills-install-malformed-json",
        Classification::RawJson,
    ),
    (
        "agent-skills-install-oversized-body",
        Classification::BodySize,
    ),
    ("agent-access-approve-missing-key", Classification::Local),
    ("agent-access-approve-invalid-key", Classification::Local),
    ("agent-access-approve-invalid-json", Classification::RawJson),
    (
        "agent-access-approve-missing-media-type",
        Classification::MediaType,
    ),
    ("agent-access-deny-missing-key", Classification::Local),
    ("agent-access-deny-invalid-key", Classification::Local),
    ("agent-access-deny-invalid-json", Classification::RawJson),
    (
        "agent-access-deny-missing-media-type",
        Classification::MediaType,
    ),
    ("settings-state-agent-invalid-input", Classification::Local),
    (
        "preferences-state-agent-invalid-input",
        Classification::Local,
    ),
    (
        "preferences-agent-input-unavailable-invalid-key",
        Classification::Local,
    ),
    (
        "preferences-agent-input-unavailable-missing-key",
        Classification::Local,
    ),
    (
        "preferences-agent-input-unavailable-invalid-body",
        Classification::TypedShape,
    ),
    (
        "preferences-agent-input-unavailable-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "preferences-agent-input-unavailable-missing-media-type",
        Classification::MediaType,
    ),
    (
        "preferences-malformed-post-api-v1-agents-id-input",
        Classification::RawJson,
    ),
    (
        "settings-agent-input-unavailable-invalid-key",
        Classification::Local,
    ),
    (
        "settings-agent-input-unavailable-missing-key",
        Classification::Local,
    ),
    (
        "settings-agent-input-unavailable-invalid-body",
        Classification::TypedShape,
    ),
    (
        "settings-agent-input-unavailable-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "settings-agent-input-unavailable-missing-media-type",
        Classification::MediaType,
    ),
    (
        "settings-malformed-post-api-v1-agents-id-input",
        Classification::RawJson,
    ),
    (
        "gateway-connection-list-invalid-query",
        Classification::TypedShape,
    ),
    (
        "gateway-connection-delete-invalid-query",
        Classification::TypedShape,
    ),
    (
        "gateway-connection-put-invalid-json",
        Classification::RawJson,
    ),
    ("queued-cancellation-v2-missing-key", Classification::Local),
    (
        "queued-cancellation-v2-invalid-revision",
        Classification::Local,
    ),
    (
        "queued-cancellation-v2-unknown-field",
        Classification::TypedShape,
    ),
    (
        "queued-cancellation-v2-invalid-json",
        Classification::RawJson,
    ),
    (
        "queued-cancellation-v2-query-observe",
        Classification::QueryShape,
    ),
    (
        "queued-cancellation-v2-query-submit",
        Classification::QueryShape,
    ),
    ("interruption-v2-missing-key", Classification::Local),
    ("interruption-v2-invalid-revision", Classification::Local),
    ("interruption-v2-empty-text", Classification::Local),
    ("interruption-v2-whitespace-text", Classification::Local),
    ("interruption-v2-null-text", Classification::TypedShape),
    ("interruption-v2-unknown-field", Classification::TypedShape),
    ("interruption-v2-invalid-json", Classification::RawJson),
    ("interruption-v2-query", Classification::QueryShape),
    ("queued-input-v2-missing-key", Classification::Local),
    ("queued-input-v2-invalid-revision", Classification::Local),
    ("queued-input-v2-empty-text", Classification::Local),
    ("queued-input-v2-whitespace-text", Classification::Local),
    ("queued-input-v2-null-text", Classification::TypedShape),
    ("queued-input-v2-unknown-field", Classification::TypedShape),
    ("queued-input-v2-invalid-json", Classification::RawJson),
    ("queued-input-v2-query-list", Classification::QueryShape),
    ("queued-input-v2-query-submit", Classification::QueryShape),
    ("agent-events-v2-snapshot-invalid", Classification::Local),
    ("agent-events-v2-head-query", Classification::QueryShape),
    ("agent-events-v2-events-invalid", Classification::Local),
    ("agent-events-v2-snapshot-query", Classification::QueryShape),
    ("agent-events-v2-events-query", Classification::QueryShape),
    (
        "agent-events-v2-snapshot-duplicate",
        Classification::QueryShape,
    ),
    (
        "agent-events-v2-events-duplicate",
        Classification::QueryShape,
    ),
    ("response-controls-v2-missing-key", Classification::Local),
    (
        "response-controls-v2-unknown-body",
        Classification::TypedShape,
    ),
    (
        "response-controls-v2-invalid-observation",
        Classification::Local,
    ),
    ("response-controls-v2-invalid-json", Classification::RawJson),
    (
        "response-controls-v2-observe-query",
        Classification::QueryShape,
    ),
    (
        "response-controls-v2-status-query",
        Classification::QueryShape,
    ),
    (
        "channel-inventory-invalid-active",
        Classification::TypedShape,
    ),
    (
        "channel-inventory-invalid-history",
        Classification::TypedShape,
    ),
    ("provider-associations-zero-limit", Classification::Local),
    ("provider-associations-large-limit", Classification::Local),
    ("channel-usage-zero-limit", Classification::Local),
    ("channel-usage-large-limit", Classification::Local),
    ("channel-usage-invalid-cursor", Classification::Local),
    ("artifact-missing-key", Classification::Local),
    ("artifact-invalid-name", Classification::Local),
    ("artifact-invalid-media_type", Classification::Local),
    ("artifact-invalid-content_base64", Classification::Local),
    ("artifact-zero-limit", Classification::Local),
    ("artifact-large-limit", Classification::Local),
    ("artifact-invalid-cursor", Classification::Local),
    (
        "shared-sandbox-inventory-mixed-scope",
        Classification::Local,
    ),
    (
        "shared-sandbox-inventory-invalid-shared",
        Classification::TypedShape,
    ),
    ("runtime-update-missing-key", Classification::Local),
    ("sandbox-command-missing-key", Classification::Local),
    ("sandbox-command-invalid-key", Classification::Local),
    ("sandbox-command-invalid-revision", Classification::Local),
    ("sandbox-command-spoofed-owner", Classification::TypedShape),
    (
        "creation-inventory-invalid-category",
        Classification::TypedShape,
    ),
    (
        "creation-inventory-invalid-creator",
        Classification::QueryShape,
    ),
    ("responses-v2-field-stream", Classification::Local),
    (
        "responses-v2-field-previous_response_id",
        Classification::Local,
    ),
    ("responses-v2-field-tools", Classification::TypedShape),
    ("responses-v2-field-store", Classification::Local),
    ("responses-v2-field-metadata", Classification::TypedShape),
    ("responses-v2-field-input", Classification::TypedShape),
    ("responses-v2-field-conversation", Classification::Local),
    ("responses-v2-alias-mismatch", Classification::Local),
    ("responses-v2-items-limit", Classification::Local),
    ("responses-v2-items-order", Classification::TypedShape),
    ("responses-v2-retrieve-query", Classification::QueryShape),
    (
        "sandbox-inventory-status-invalid",
        Classification::TypedShape,
    ),
    (
        "sandbox-inventory-invalid-owner",
        Classification::QueryShape,
    ),
    (
        "org-inventory-invalid-authority-injection",
        Classification::QueryShape,
    ),
    ("agent-service-access-missing-key", Classification::Local),
    ("agent-service-access-invalid-key", Classification::Local),
    ("agent-service-access-invalid-json", Classification::RawJson),
    (
        "agent-service-access-missing-media-type",
        Classification::MediaType,
    ),
    ("sandbox-service-access-missing-key", Classification::Local),
    ("sandbox-service-access-invalid-key", Classification::Local),
    (
        "sandbox-service-access-invalid-json",
        Classification::RawJson,
    ),
    (
        "sandbox-service-access-content-type",
        Classification::MediaType,
    ),
    ("sandbox-service-access-oversize", Classification::BodySize),
    ("authoring-skills-limit-zero", Classification::Local),
    (
        "authoring-skills-limit-overflow",
        Classification::TypedShape,
    ),
    ("authoring-skills-limit-too-large", Classification::Local),
    ("authoring-skills-limit-text", Classification::TypedShape),
    ("authoring-skills-query-too-long", Classification::Local),
    ("authoring-skills-query-control", Classification::Local),
    ("authoring-skills-query-byte-limit", Classification::Local),
    ("authoring-skills-cursor-empty", Classification::Local),
    ("authoring-skills-cursor-too-long", Classification::Local),
    ("authoring-skills-cursor-control", Classification::Local),
    ("authoring-skills-unknown-query", Classification::QueryShape),
    (
        "authoring-skills-duplicate-query",
        Classification::QueryShape,
    ),
    (
        "authoring-skills-forbidden-before-query",
        Classification::TypedShape,
    ),
    (
        "authoring-skills-anonymous-before-query",
        Classification::TypedShape,
    ),
    ("authoring-resolve-invalid-json", Classification::RawJson),
    ("authoring-resolve-body-too-large", Classification::BodySize),
    (
        "authoring-resolve-missing-content-type",
        Classification::MediaType,
    ),
    (
        "authoring-resolve-wrong-content-type",
        Classification::MediaType,
    ),
    (
        "authoring-resolve-invalid-body-type",
        Classification::TypedShape,
    ),
    (
        "authoring-resolve-unknown-field",
        Classification::TypedShape,
    ),
    (
        "authoring-resolve-forbidden-before-body",
        Classification::RawJson,
    ),
    (
        "authoring-resolve-anonymous-before-body",
        Classification::RawJson,
    ),
    ("oversized-admin-put", Classification::BodySize),
    ("oversized-admin-patch", Classification::BodySize),
    ("oversized-admin-post", Classification::BodySize),
    ("launch-error-create-missing-key", Classification::Local),
    ("launch-error-create-invalid-key", Classification::Local),
    (
        "launch-error-create-invalid-body",
        Classification::TypedShape,
    ),
    (
        "launch-error-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "launch-error-create-malformed-body",
        Classification::RawJson,
    ),
    ("launch-error-start-missing-key", Classification::Local),
    ("launch-error-start-invalid-key", Classification::Local),
    (
        "launch-error-start-invalid-body",
        Classification::TypedShape,
    ),
    (
        "launch-error-start-missing-media-type",
        Classification::MediaType,
    ),
    ("launch-error-start-malformed-body", Classification::RawJson),
    ("launch-error-cancel-missing-key", Classification::Local),
    ("launch-error-cancel-invalid-key", Classification::Local),
    (
        "launch-error-connection-create-missing-key",
        Classification::Local,
    ),
    (
        "launch-error-connection-create-invalid-key",
        Classification::Local,
    ),
    (
        "launch-error-connection-create-invalid-body",
        Classification::TypedShape,
    ),
    (
        "launch-error-connection-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "launch-error-connection-create-malformed-body",
        Classification::RawJson,
    ),
    (
        "launch-error-create-missing-start-choice",
        Classification::TypedShape,
    ),
    (
        "launch-error-list-invalid-state",
        Classification::TypedShape,
    ),
    (
        "launch-error-list-malformed-limit",
        Classification::TypedShape,
    ),
    (
        "launch-error-events-malformed-after",
        Classification::TypedShape,
    ),
    (
        "launch-error-stream-invalid-query",
        Classification::QueryShape,
    ),
    (
        "launch-error-stream-malformed-follow",
        Classification::TypedShape,
    ),
    ("launch-error-stream-invalid-limit", Classification::Local),
    ("state-sandbox-create-invalid", Classification::TypedShape),
    ("state-sandbox-update-invalid", Classification::TypedShape),
    (
        "agent-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-steer-no-turn-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-approval-not-pending-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-input-unavailable-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-stop-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "sandbox-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-profile-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-profile-update-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-profile-unpublished-public-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-profile-publish-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "agent-marketplace-fork-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-update-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-unpublished-public-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-publish-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "sandbox-profile-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "sandbox-profile-update-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "sandbox-profile-unpublished-public-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "sandbox-profile-publish-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "malformed-patch-api-v1-admin-settings",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-admin-settings-reset",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agent-profiles",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agent-profiles-id-revisions",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agents-id-approvals-request-id",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agents-id-input",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agents-id-steer",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-agents-id-stop",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-marketplace-kind-id-fork",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-sandbox-profiles",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-sandbox-profiles-id-revisions",
        Classification::RawJson,
    ),
    ("malformed-post-api-v1-sandboxes", Classification::RawJson),
    (
        "malformed-post-api-v1-sandboxes-id-workspaces",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-workspace-profiles",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-workspace-profiles-id-revisions",
        Classification::RawJson,
    ),
    (
        "malformed-post-api-v1-workspaces-id-agents",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-admin-settings",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-agent-profiles-id",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-agent-profiles-id-visibility",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-sandbox-profiles-id",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-sandbox-profiles-id-visibility",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-workspace-profiles-id",
        Classification::RawJson,
    ),
    (
        "malformed-put-api-v1-workspace-profiles-id-visibility",
        Classification::RawJson,
    ),
    (
        "trigger-error-create-invalid-body",
        Classification::TypedShape,
    ),
    ("trigger-error-create-invalid-key", Classification::Local),
    ("trigger-error-create-missing-key", Classification::Local),
    (
        "trigger-error-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "trigger-error-delete-published-refused-invalid-key",
        Classification::Local,
    ),
    (
        "trigger-error-delete-published-refused-missing-key",
        Classification::Local,
    ),
    (
        "trigger-error-list-malformed-query",
        Classification::TypedShape,
    ),
    (
        "trigger-error-placement-create-invalid-body",
        Classification::TypedShape,
    ),
    (
        "trigger-error-placement-create-invalid-key",
        Classification::Local,
    ),
    (
        "trigger-error-placement-create-missing-key",
        Classification::Local,
    ),
    (
        "trigger-error-placement-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "trigger-error-placement-retire-invalid-key",
        Classification::Local,
    ),
    (
        "trigger-error-placement-retire-missing-key",
        Classification::Local,
    ),
    (
        "trigger-error-placements-list-invalid-query",
        Classification::TypedShape,
    ),
    (
        "trigger-error-publish-invalid-body",
        Classification::TypedShape,
    ),
    ("trigger-error-publish-invalid-key", Classification::Local),
    ("trigger-error-publish-missing-key", Classification::Local),
    (
        "trigger-error-publish-missing-media-type",
        Classification::MediaType,
    ),
    ("trigger-error-retire-invalid-key", Classification::Local),
    ("trigger-error-retire-missing-key", Classification::Local),
    (
        "trigger-error-save-invalid-body",
        Classification::TypedShape,
    ),
    ("trigger-error-save-invalid-key", Classification::Local),
    ("trigger-error-save-missing-key", Classification::Local),
    (
        "trigger-error-save-missing-media-type",
        Classification::MediaType,
    ),
    (
        "admin-activity-empty-filter-invalid-page",
        Classification::Local,
    ),
    ("admin-invalid-query", Classification::Local),
    ("admin-profile-list-invalid-page", Classification::Local),
    ("admin-sandbox-events-invalid-page", Classification::Local),
    ("admin-sandbox-list-invalid-page", Classification::Local),
    ("admin-settings-invalid", Classification::Local),
    (
        "admin-settings-patch-invalid-body",
        Classification::TypedShape,
    ),
    (
        "admin-settings-patch-missing-media-type",
        Classification::MediaType,
    ),
    (
        "admin-settings-replace-invalid-body",
        Classification::TypedShape,
    ),
    (
        "admin-settings-replace-missing-media-type",
        Classification::MediaType,
    ),
    (
        "admin-settings-reset-invalid-body",
        Classification::TypedShape,
    ),
    (
        "admin-settings-reset-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-approval-not-pending-invalid-body",
        Classification::TypedShape,
    ),
    (
        "agent-approval-not-pending-invalid-key",
        Classification::Local,
    ),
    (
        "agent-approval-not-pending-missing-key",
        Classification::Local,
    ),
    (
        "agent-approval-not-pending-missing-media-type",
        Classification::MediaType,
    ),
    ("agent-create-invalid-body", Classification::TypedShape),
    ("agent-create-invalid-key", Classification::Local),
    ("agent-create-missing-key", Classification::Local),
    ("agent-create-missing-media-type", Classification::MediaType),
    (
        "agent-delete-not-stopped-invalid-key",
        Classification::Local,
    ),
    (
        "agent-delete-not-stopped-missing-key",
        Classification::Local,
    ),
    (
        "agent-input-unavailable-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-input-unavailable-invalid-key", Classification::Local),
    ("agent-input-unavailable-missing-key", Classification::Local),
    (
        "agent-input-unavailable-missing-media-type",
        Classification::MediaType,
    ),
    ("agent-interrupt-no-turn-invalid-key", Classification::Local),
    ("agent-interrupt-no-turn-missing-key", Classification::Local),
    (
        "agent-marketplace-fork-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-marketplace-fork-invalid-key", Classification::Local),
    ("agent-marketplace-fork-missing-key", Classification::Local),
    (
        "agent-marketplace-fork-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-profile-create-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-profile-create-invalid-key", Classification::Local),
    ("agent-profile-create-missing-key", Classification::Local),
    (
        "agent-profile-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-profile-publish-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-profile-publish-invalid-key", Classification::Local),
    ("agent-profile-publish-missing-key", Classification::Local),
    (
        "agent-profile-publish-missing-media-type",
        Classification::MediaType,
    ),
    ("agent-profile-retire-invalid-key", Classification::Local),
    ("agent-profile-retire-missing-key", Classification::Local),
    (
        "agent-profile-unpublished-public-invalid-body",
        Classification::TypedShape,
    ),
    (
        "agent-profile-unpublished-public-invalid-key",
        Classification::Local,
    ),
    (
        "agent-profile-unpublished-public-missing-key",
        Classification::Local,
    ),
    (
        "agent-profile-unpublished-public-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-profile-update-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-profile-update-invalid-key", Classification::Local),
    ("agent-profile-update-missing-key", Classification::Local),
    (
        "agent-profile-update-missing-media-type",
        Classification::MediaType,
    ),
    (
        "agent-steer-no-turn-invalid-body",
        Classification::TypedShape,
    ),
    ("agent-steer-no-turn-invalid-key", Classification::Local),
    ("agent-steer-no-turn-missing-key", Classification::Local),
    (
        "agent-steer-no-turn-missing-media-type",
        Classification::MediaType,
    ),
    ("agent-stop-invalid-body", Classification::TypedShape),
    ("agent-stop-invalid-key", Classification::Local),
    ("agent-stop-missing-key", Classification::Local),
    ("agent-stop-missing-media-type", Classification::MediaType),
    ("agui-empty-run", Classification::Local),
    ("agui-invalid-body", Classification::TypedShape),
    ("agui-invalid-replay-query", Classification::QueryShape),
    ("agui-invalid-run", Classification::Local),
    ("agui-missing-content-type", Classification::MediaType),
    ("agui-profile-override-refused", Classification::TypedShape),
    ("profile-invalid-content", Classification::TypedShape),
    (
        "responses-conversation-content-type",
        Classification::MediaType,
    ),
    (
        "responses-conversation-invalid-body",
        Classification::TypedShape,
    ),
    (
        "responses-conversation-metadata-refusal",
        Classification::TypedShape,
    ),
    ("responses-input-role", Classification::TypedShape),
    ("responses-instructions-null", Classification::TypedShape),
    ("responses-items-bad-limit", Classification::Local),
    ("responses-items-bad-query", Classification::QueryShape),
    ("responses-metadata-values", Classification::TypedShape),
    ("responses-missing-content-type", Classification::MediaType),
    ("responses-read-bad-query", Classification::QueryShape),
    ("responses-read-stream-refused", Classification::QueryShape),
    ("responses-stateless", Classification::TypedShape),
    ("responses-store-false", Classification::Local),
    ("responses-stream-false", Classification::Local),
    ("responses-tools-null", Classification::TypedShape),
    ("sandbox-create-invalid-body", Classification::TypedShape),
    ("sandbox-create-invalid-key", Classification::Local),
    ("sandbox-create-missing-key", Classification::Local),
    (
        "sandbox-create-missing-media-type",
        Classification::MediaType,
    ),
    ("sandbox-delete-invalid-key", Classification::Local),
    ("sandbox-delete-missing-key", Classification::Local),
    ("sandbox-invalid-query", Classification::TypedShape),
    ("sandbox-pause-invalid-key", Classification::Local),
    ("sandbox-pause-missing-key", Classification::Local),
    (
        "sandbox-profile-create-invalid-body",
        Classification::TypedShape,
    ),
    ("sandbox-profile-create-invalid-key", Classification::Local),
    ("sandbox-profile-create-missing-key", Classification::Local),
    (
        "sandbox-profile-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "sandbox-profile-publish-invalid-body",
        Classification::TypedShape,
    ),
    ("sandbox-profile-publish-invalid-key", Classification::Local),
    ("sandbox-profile-publish-missing-key", Classification::Local),
    (
        "sandbox-profile-publish-missing-media-type",
        Classification::MediaType,
    ),
    ("sandbox-profile-retire-invalid-key", Classification::Local),
    ("sandbox-profile-retire-missing-key", Classification::Local),
    (
        "sandbox-profile-unpublished-public-invalid-body",
        Classification::TypedShape,
    ),
    (
        "sandbox-profile-unpublished-public-invalid-key",
        Classification::Local,
    ),
    (
        "sandbox-profile-unpublished-public-missing-key",
        Classification::Local,
    ),
    (
        "sandbox-profile-unpublished-public-missing-media-type",
        Classification::MediaType,
    ),
    (
        "sandbox-profile-update-invalid-body",
        Classification::TypedShape,
    ),
    ("sandbox-profile-update-invalid-key", Classification::Local),
    ("sandbox-profile-update-missing-key", Classification::Local),
    (
        "sandbox-profile-update-missing-media-type",
        Classification::MediaType,
    ),
    (
        "sandbox-revive-not-paused-invalid-key",
        Classification::Local,
    ),
    (
        "sandbox-revive-not-paused-missing-key",
        Classification::Local,
    ),
    ("triggers-badge-invalid-query", Classification::TypedShape),
    ("triggers-embed-invalid-query", Classification::QueryShape),
    ("workspace-create-invalid-body", Classification::TypedShape),
    ("workspace-create-invalid-key", Classification::Local),
    ("workspace-create-missing-key", Classification::Local),
    (
        "workspace-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "workspace-live-agent-conflict-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-live-agent-conflict-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-create-invalid-body",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-create-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-profile-create-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-create-missing-media-type",
        Classification::MediaType,
    ),
    (
        "workspace-profile-publish-invalid-body",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-publish-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-profile-publish-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-publish-missing-media-type",
        Classification::MediaType,
    ),
    (
        "workspace-profile-retire-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-profile-retire-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-unpublished-public-invalid-body",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-unpublished-public-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-profile-unpublished-public-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-unpublished-public-missing-media-type",
        Classification::MediaType,
    ),
    (
        "workspace-profile-update-invalid-body",
        Classification::TypedShape,
    ),
    (
        "workspace-profile-update-invalid-key",
        Classification::Local,
    ),
    (
        "workspace-profile-update-missing-key",
        Classification::Local,
    ),
    (
        "workspace-profile-update-missing-media-type",
        Classification::MediaType,
    ),
];

pub fn classify(case: &Case) -> Option<Classification> {
    LOCAL_CASES
        .iter()
        .find(|(id, _)| *id == case.id)
        .map(|(_, kind)| *kind)
}

pub fn assert_unrepresentable(case: &Case, class: Classification) {
    match class {
        Classification::BinaryHeaders => {
            let headers = &case.request.headers;
            let invalid = match case.id.as_str() {
                "binding-media-missing-length" => !headers.contains_key("content-length"),
                "binding-media-missing-type" => !headers.contains_key("content-type"),
                "binding-media-encoding" => {
                    headers.get("content-encoding").map(String::as_str) == Some("gzip")
                }
                "binding-media-too-large" => {
                    headers["content-length"].parse::<usize>().unwrap() > 20 * 1024 * 1024
                }
                "binding-media-incomplete-body" => {
                    headers["content-length"].parse::<usize>().unwrap()
                        != case.request.expanded_raw_body().unwrap().len()
                }
                _ => false,
            };
            assert!(
                invalid,
                "{} typed binary upload derives exact headers",
                case.id
            );
        }
        Classification::MediaType => {
            let content_type = case.request.headers.get("content-type").map(String::as_str);
            assert_eq!(
                content_type,
                (case.id == "authoring-resolve-wrong-content-type").then_some("text/plain"),
                "{}",
                case.id
            );
        }
        Classification::RawJson => assert!(
            serde_json::from_str::<serde_json::Value>(&case.request.expanded_raw_body().unwrap())
                .is_err(),
            "{}",
            case.id
        ),
        Classification::BodySize => {
            let raw = case.request.expanded_raw_body().unwrap();
            assert!(raw.len() > 2 * 1024 * 1024, "{}", case.id);
            assert!(raw.bytes().all(|byte| byte == b' '), "{}", case.id);
            assert_eq!(case.response.status, 413, "{}", case.id);
        }
        Classification::QueryShape => {
            let url = reqwest::Url::parse(&format!("http://fixture{}", case.request.path)).unwrap();
            if case.id.starts_with("agent-events-v2-") && case.id.ends_with("-duplicate") {
                let key = if case.id.contains("snapshot") {
                    "page_cursor"
                } else {
                    "after_cursor"
                };
                assert_eq!(url.query_pairs().filter(|(name, _)| name == key).count(), 2);
                return;
            }
            if case.id == "authoring-skills-duplicate-query" {
                assert_eq!(url.query_pairs().filter(|(key, _)| key == "q").count(), 2);
                return;
            }
            let allowed: &[&str] = match case.operation.as_str() {
                "GET /api/v1/agents/{id}/files/tree"
                | "GET /api/v1/agents/{id}/files/content"
                | "GET /api/v1/agents/{id}/files/download"
                | "DELETE /api/v1/agents/{id}/files" => &["path", "max_bytes", "recursive"],
                "GET /api/v2/agents/{agent_id}/events/snapshot" => &["limit", "page_cursor"],
                "GET /api/v2/agents/{agent_id}/events/head" => &[],
                "GET /api/v2/agents/{agent_id}/events" => &["after_cursor", "follow"],
                "GET /api/v2/agents/{agent_id}/queued-inputs" => &["after", "limit"],
                "GET /api/v1/bridge-connectors/capabilities"
                | "GET /api/v1/bridge-creation-target" => &["organization"],
                "GET /api/v1/bridges" => &["organization", "cursor", "limit"],
                "GET /api/v1/agents/provenance" => &[
                    "organization",
                    "sandbox",
                    "workspace",
                    "status",
                    "creation_origin",
                    "cursor",
                    "limit",
                ],
                "GET /api/v1/sandboxes" => &["organization", "status", "cursor", "limit"],
                "GET /api/v1/agents" => &[
                    "organization",
                    "sandbox",
                    "workspace",
                    "status",
                    "cursor",
                    "limit",
                ],
                "GET /api/v1/profile-authoring/skills" => &["q", "limit", "cursor"],
                "GET /api/v1/agui/events" => &["thread_id", "starting_after", "follow"],
                "GET /api/v1/conversations/{id}/items" => &["after", "limit"],
                "GET /api/v1/triggers/{id}/embed" => &["placement_id"],
                "GET /api/v1/agui/launches/{id}/events" => &["starting_after", "limit", "follow"],
                "GET /api/v1/responses/{response_id}"
                | "GET /api/v2/responses/{response_id}"
                | "GET /api/v2/agents/{agent_id}/responses/{response_id}/controls"
                | "GET /api/v2/commands/{command_id}"
                | "GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt" => &[],
                _ => panic!("unclassified query shape {}", case.id),
            };
            assert!(
                url.query_pairs()
                    .any(|(key, _)| !allowed.contains(&key.as_ref())),
                "{}",
                case.id
            );
        }
        Classification::Local | Classification::TypedShape => {
            panic!("input cases must exercise typed SDK construction or preflight")
        }
    }
}
