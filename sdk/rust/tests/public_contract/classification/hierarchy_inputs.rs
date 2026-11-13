use super::Classification;

pub(super) const CASES: &[(&str, Classification)] = &[
    ("preparation-inspect-invalid-json", Classification::RawJson),
    ("preparation-reserve-invalid-json", Classification::RawJson),
    ("preparation-reserve-missing-key", Classification::Local),
    (
        "workspace-prepare-recovery-missing-key",
        Classification::Local,
    ),
    (
        "workspace-prepare-recovery-invalid-key",
        Classification::Local,
    ),
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
    (
        "sandbox-create-invalid-body-reused-key",
        Classification::TypedShape,
    ),
    (
        "workspace-create-invalid-body-reused-key",
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
    ("profile-invalid-content", Classification::TypedShape),
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
