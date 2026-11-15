use super::Classification;

pub(super) const CASES: &[(&str, Classification)] = &[
    ("admin-admission-stop-invalid", Classification::Local),
    ("admin-admission-resume-invalid", Classification::Local),
    ("desktop-capability-owner-400", Classification::QueryShape),
    ("usage-sandboxes-zero-limit", Classification::Local),
    ("usage-admin-sandboxes-zero-limit", Classification::Local),
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
    ("state-sandbox-create-invalid", Classification::TypedShape),
    ("state-sandbox-update-invalid", Classification::TypedShape),
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
];
