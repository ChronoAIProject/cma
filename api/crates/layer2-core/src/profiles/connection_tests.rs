use super::*;

fn content() -> SandboxProfileContent {
    sandbox_profile(
        "sbp_connections",
        "owner",
        WorkspaceProfileRef {
            id: WorkspaceProfileId("wkp_connections".into()),
            revision: 1,
        },
    )
    .draft
}

#[test]
fn subscription_requires_oauth_without_granting_managed_model_services() {
    let mut profile = content();
    profile.model_access = ModelAccess::OwnCredentials;
    profile.nyxid_services = ["chrono-llm", "api-github", "ornn-api", "chrono-llm-public"]
        .map(ServiceId::new)
        .to_vec();
    let original = profile.clone();
    let original_digest = content_digest(&profile).unwrap();
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", CODEX_SUBSCRIPTION_CONNECTION].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(
            &[PUBLIC_CODEX_SERVICE, "chrono-llm", "ornn-api", "storage"].map(ServiceId::new)
        ),
        ["api-github", "ornn-api", "storage"].map(ServiceId::new)
    );
    assert_eq!(profile, original);
    assert_eq!(content_digest(&profile).unwrap(), original_digest);
}

#[test]
fn managed_provider_is_required_even_when_absent_from_the_service_picker() {
    let mut profile = content();
    profile.nyxid_services = ["api-github", "ornn-api"].map(ServiceId::new).to_vec();
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", PUBLIC_CODEX_SERVICE].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(&[]),
        ["api-github", PUBLIC_CODEX_SERVICE, "ornn-api"].map(ServiceId::new)
    );
    profile.nyxid_services.push(ServiceId::new("chrono-llm"));
    profile
        .nyxid_services
        .push(ServiceId::new(PUBLIC_CODEX_SERVICE));
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", PUBLIC_CODEX_SERVICE].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(&[ServiceId::new(PUBLIC_CODEX_SERVICE)]),
        ["api-github", PUBLIC_CODEX_SERVICE, "ornn-api"].map(ServiceId::new)
    );
}
