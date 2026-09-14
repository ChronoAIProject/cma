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
    let provider = profile.agent_provider.connection_services();
    profile.model_access = ModelAccess::OwnCredentials;
    profile.nyxid_services = ["chrono-llm", "api-github", "ornn-api", "chrono-llm-public"]
        .map(ServiceId::new)
        .to_vec();
    let original = profile.clone();
    let original_digest = content_digest(&profile).unwrap();
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", provider.subscription].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(
            &[
                provider.managed,
                provider.legacy_managed,
                "ornn-api",
                "storage"
            ]
            .map(ServiceId::new)
        ),
        ["api-github", "ornn-api", "storage"].map(ServiceId::new)
    );
    assert_eq!(profile, original);
    assert_eq!(content_digest(&profile).unwrap(), original_digest);
}

#[test]
fn managed_provider_is_required_even_when_absent_from_the_service_picker() {
    let mut profile = content();
    let provider = profile.agent_provider.connection_services();
    profile.nyxid_services = ["api-github", "ornn-api"].map(ServiceId::new).to_vec();
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", provider.managed].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(&[]),
        ["api-github", provider.managed, "ornn-api"].map(ServiceId::new)
    );
    profile.nyxid_services.push(ServiceId::new("chrono-llm"));
    profile
        .nyxid_services
        .push(ServiceId::new(provider.managed));
    assert_eq!(
        profile.connection_services(),
        ["api-github", "ornn-api", provider.managed].map(ServiceId::new)
    );
    assert_eq!(
        profile.credential_services(&[ServiceId::new(provider.managed)]),
        ["api-github", provider.managed, "ornn-api"].map(ServiceId::new)
    );
}

#[test]
fn existing_grants_retain_integrations_and_legacy_route_until_migrated() {
    let mut profile = content();
    let provider = profile.agent_provider.connection_services();
    profile.nyxid_services = ["api-github", "ornn-api"].map(ServiceId::new).to_vec();
    let mut grants = ["ornn-api", "storage", provider.legacy_managed]
        .map(ServiceId::new)
        .to_vec();
    assert_eq!(
        profile.connection_services_for_grants(&grants),
        ["ornn-api", "storage", provider.legacy_managed].map(ServiceId::new)
    );
    grants.push(ServiceId::new(provider.managed));
    assert_eq!(
        profile.connection_services_for_grants(&grants),
        ["ornn-api", "storage", provider.managed].map(ServiceId::new)
    );
    profile.model_access = ModelAccess::OwnCredentials;
    assert_eq!(
        profile.connection_services_for_grants(&grants),
        ["ornn-api", "storage", provider.subscription].map(ServiceId::new)
    );
}
