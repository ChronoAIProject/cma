use super::*;
use crate::profiles::ProfileVisibility;

const RETIRED_AT: &str = "2026-11-14T00:00:00Z";

#[test]
fn canonical_retained_configuration_uses_neutral_disclosure_labels() {
    for (resource, expected) in [
        (LaunchTrustResource::Sandbox, "Sandbox"),
        (LaunchTrustResource::Workspace, "Workspace"),
    ] {
        assert_eq!(
            disclosure_name(
                "",
                "",
                &SubjectId::new(crate::DELETED_ACCOUNT_SUBJECT),
                ProfileVisibility::Private,
                Some(RETIRED_AT),
                false,
                resource,
            ),
            expected,
        );
    }
}

#[test]
fn ordinary_names_are_preserved_and_partial_erasure_does_not_supply_a_label() {
    for (name, description, owner, visibility, retired_at, has_fork) in [
        (
            "",
            "",
            "ordinary-owner",
            ProfileVisibility::Private,
            Some(RETIRED_AT),
            false,
        ),
        (
            "",
            "",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Public,
            Some(RETIRED_AT),
            false,
        ),
        (
            "",
            "",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Private,
            None,
            false,
        ),
        (
            "",
            "",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Private,
            Some("invalid"),
            false,
        ),
        (
            "",
            "unremoved description",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Private,
            Some(RETIRED_AT),
            false,
        ),
        (
            "",
            "",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Private,
            Some(RETIRED_AT),
            true,
        ),
        (
            " ",
            "",
            crate::DELETED_ACCOUNT_SUBJECT,
            ProfileVisibility::Private,
            Some(RETIRED_AT),
            false,
        ),
        (
            "Original name",
            "",
            "ordinary-owner",
            ProfileVisibility::Public,
            None,
            false,
        ),
    ] {
        for resource in [LaunchTrustResource::Sandbox, LaunchTrustResource::Workspace] {
            assert_eq!(
                disclosure_name(
                    name,
                    description,
                    &SubjectId::new(owner),
                    visibility,
                    retired_at,
                    has_fork,
                    resource
                ),
                name,
            );
        }
    }
}

#[test]
fn empty_or_whitespace_disclosure_labels_remain_invalid() {
    let digest = "a".repeat(64);
    for name in ["", " ", "\t"] {
        let manifest = LaunchTrustManifest {
            version: 1,
            confirmation_digest: Some(digest.clone()),
            sandbox_digest: Some(digest.clone()),
            workspace_digest: None,
            disclosures: vec![LaunchTrustDisclosure {
                resource: LaunchTrustResource::Sandbox,
                name: name.into(),
                commands_digest: digest.clone(),
                commands: vec!["prepare-environment".into()],
                environment_variables: vec![],
            }],
        };
        assert!(manifest.validate().is_err());
    }
}
