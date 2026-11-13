use super::Classification;

pub(super) const CASES: &[(&str, Classification)] = &[
    ("deliverables-owner-invalid-revision", Classification::Local),
    (
        "deliverables-owner-invalid-answer-list",
        Classification::Local,
    ),
    (
        "deliverables-owner-invalid-answer-read",
        Classification::Local,
    ),
    ("suggestions-invalid_query", Classification::TypedShape),
    ("feedback-reasons-invalid-comment", Classification::Local),
    ("feedback-reasons-invalid-message", Classification::Local),
    (
        "feedback-reasons-unknown-reason",
        Classification::TypedShape,
    ),
    ("agent-worktree-recovery-missing-key", Classification::Local),
    ("agent-worktree-recovery-invalid-key", Classification::Local),
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
    ("agent-service-access-missing-key", Classification::Local),
    ("agent-service-access-invalid-key", Classification::Local),
    ("agent-service-access-invalid-json", Classification::RawJson),
    (
        "agent-service-access-missing-media-type",
        Classification::MediaType,
    ),
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
];
