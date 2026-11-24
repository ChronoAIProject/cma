use super::*;
use crate::auth::{identity_flight::IdentityStore, profile::Profile};
use std::time::{Duration, SystemTime};

fn context() -> (std::sync::Arc<AuthState>, ProviderMetadata) {
    let auth = AuthState::in_memory(
        AuthMode::Provider {
            base_url: "https://provider.example.test".into(),
            client_id: "fixture-client".into(),
            client_secret: String::new(),
            public_url: "https://app.example.test".into(),
        },
        None,
    );
    let metadata = ProviderMetadata {
        authorization_endpoint: "https://provider.example.test/authorize".into(),
        token_endpoint: "https://provider.example.test/token".into(),
        userinfo_endpoint: Some("https://provider.example.test/userinfo".into()),
        introspection_endpoint: Some("https://provider.example.test/introspect".into()),
    };
    (auth, metadata)
}

fn evidence() -> VerifiedSubject {
    VerifiedSubject {
        subject: layer2_core::SubjectId::new("synthetic-owner"),
        permissions: PermissionEvidence::Active(vec!["cma:sandbox:use".into()]),
        profile: Profile::default(),
        independent_commands: false,
        api_expires_at: None,
        token_expires_at: None,
        verification: None,
    }
}

pub(in crate::auth) fn fixture_stamp(
    store: &dyn IdentityStore,
    key: &TokenHash,
    permissions: &[&str],
) -> EvidenceStamp {
    let (auth, metadata) = context();
    let mut evidence = evidence();
    evidence.permissions =
        PermissionEvidence::Active(permissions.iter().map(|s| s.to_string()).collect());
    store
        .evidence_stamp(
            key,
            EvidenceScope::new(&auth, &metadata, "fixture"),
            &evidence,
        )
        .unwrap()
}

#[test]
fn stamps_bind_exact_authority_and_expiry_but_not_display_or_permission_order() {
    let (auth, metadata) = context();
    let key = auth.identity_store.key("fixture");
    let make = |evidence: &VerifiedSubject| {
        auth.identity_store
            .evidence_stamp(
                &key,
                EvidenceScope::new(&auth, &metadata, "fixture"),
                evidence,
            )
            .unwrap()
    };
    let mut original = evidence();
    original.permissions =
        PermissionEvidence::Active(vec!["permission-a".into(), "permission-b".into()]);
    let first = make(&original);
    original.permissions = PermissionEvidence::Active(vec![
        "permission-b".into(),
        "permission-a".into(),
        "permission-b".into(),
    ]);
    original.profile.display_name = Some("Different display".into());
    original.profile.email = Some("display@example.test".into());
    original.profile.avatar_url = Some("https://display.example.test/picture".into());
    assert!(first == make(&original));
    for mutate in [
        |e: &mut VerifiedSubject| e.subject = layer2_core::SubjectId::new("another-owner"),
        |e: &mut VerifiedSubject| e.permissions = PermissionEvidence::Inactive,
        |e: &mut VerifiedSubject| e.permissions = PermissionEvidence::Active(vec![]),
        |e: &mut VerifiedSubject| {
            e.permissions = PermissionEvidence::Active(vec!["permission-a".into()])
        },
        |e: &mut VerifiedSubject| {
            e.permissions =
                PermissionEvidence::Active(vec!["permission-a ".into(), "permission-b".into()])
        },
        |e: &mut VerifiedSubject| e.independent_commands = true,
        |e: &mut VerifiedSubject| {
            e.api_expires_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5))
        },
        |e: &mut VerifiedSubject| {
            e.token_expires_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5))
        },
    ] {
        let mut changed = evidence();
        changed.permissions =
            PermissionEvidence::Active(vec!["permission-a".into(), "permission-b".into()]);
        mutate(&mut changed);
        assert!(first != make(&changed));
    }
    let mut timed = evidence();
    for api in [false, true] {
        let target = if api {
            &mut timed.api_expires_at
        } else {
            &mut timed.token_expires_at
        };
        *target = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(20));
        let at_twenty = make(&timed);
        for offset in [Duration::new(19, 999_999_999), Duration::new(20, 1)] {
            let target = if api {
                &mut timed.api_expires_at
            } else {
                &mut timed.token_expires_at
            };
            *target = Some(SystemTime::UNIX_EPOCH + offset);
            assert!(at_twenty != make(&timed));
        }
    }
    timed.api_expires_at = Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1));
    assert!(matches!(
        auth.identity_store.evidence_stamp(
            &key,
            EvidenceScope::new(&auth, &metadata, "fixture"),
            &timed
        ),
        Err(Error::Unavailable(_))
    ));
}

#[test]
fn token_provider_resource_and_process_namespaces_are_distinct() {
    let (mut auth, mut metadata) = context();
    let store = auth.identity_store.clone();
    let key = store.key("fixture");
    let evidence = evidence();
    let make = |auth: &AuthState, metadata: &ProviderMetadata, token: &str| {
        store
            .evidence_stamp(&key, EvidenceScope::new(auth, metadata, token), &evidence)
            .unwrap()
    };
    let original = make(&auth, &metadata, "fixture");
    assert!(original != make(&auth, &metadata, "nyx_fixture"));
    assert!(
        original
            != store
                .evidence_stamp(
                    &store.key("another"),
                    EvidenceScope::new(&auth, &metadata, "fixture"),
                    &evidence
                )
                .unwrap()
    );
    for change in [
        |m: &mut ProviderMetadata| m.authorization_endpoint.push_str("/changed"),
        |m: &mut ProviderMetadata| m.token_endpoint.push_str("/changed"),
        |m: &mut ProviderMetadata| m.userinfo_endpoint = None,
        |m: &mut ProviderMetadata| m.introspection_endpoint = None,
    ] {
        let mut changed = metadata.clone();
        change(&mut changed);
        assert!(original != make(&auth, &changed, "fixture"));
    }
    metadata.userinfo_endpoint = Some("https://other.example.test/userinfo".into());
    assert!(original != make(&auth, &metadata, "fixture"));
    let (_, metadata) = context();
    for change in [
        |a: &mut AuthState| a.oauth_api_resource = Some("https://resource.example.test".into()),
        |a: &mut AuthState| a.agent_key_service_id = Some("other-service".into()),
        |a: &mut AuthState| {
            if let AuthMode::Provider { base_url, .. } = &mut a.mode {
                base_url.push_str("/issuer");
            }
        },
        |a: &mut AuthState| {
            if let AuthMode::Provider { client_id, .. } = &mut a.mode {
                client_id.push_str("-tenant");
            }
        },
    ] {
        let (mut other, _) = context();
        change(std::sync::Arc::get_mut(&mut other).unwrap());
        assert!(original != make(&other, &metadata, "fixture"));
    }
    let (other, _) = context();
    assert!(
        original
            != other
                .identity_store
                .evidence_stamp(
                    &key,
                    EvidenceScope::new(&other, &metadata, "fixture"),
                    &evidence
                )
                .unwrap()
    );
    std::sync::Arc::get_mut(&mut auth).unwrap().mode = AuthMode::Trusted {
        subject: "trusted".into(),
    };
    assert!(original != make(&auth, &metadata, "fixture"));
}
