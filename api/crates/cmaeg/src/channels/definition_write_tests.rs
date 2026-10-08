//! Management write-switch and size refusals at their shared storage boundary.
use super::{
    management::seal_definition,
    routing_fixture::{definition as fixture_definition, digest_of, routed},
    state::{ManagementError, ManagementState},
    write_gate,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg_core::{
    channel::{ChannelDefinition, Extensions, Secret},
    profile::{canonical_json, MAX_SYSTEM_PROMPT_BYTES},
    seal::{open_blob, MasterKeyRing, MAX_BLOB_BYTES},
    ValidationError,
};
use std::sync::Arc;

fn ring() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD_NO_PAD.encode([1; 32]))).unwrap()
}

fn oversized(extended: bool) -> ChannelDefinition {
    let mut value = routed();
    if !extended {
        value.as_object_mut().unwrap().remove("routing");
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
    }
    // JSON escaping exceeds the blob bound while the profile remains valid.
    value["profile"]["context"]["system_prompt"] = "\"".repeat(MAX_SYSTEM_PROMPT_BYTES).into();
    value["profile_digest"] = digest_of(&value["profile"]).into();
    let definition = fixture_definition(value);
    definition.validate().unwrap();
    assert_eq!(Extensions::of(&definition).legacy_compatible(), !extended);
    definition
}

#[test]
fn legacy_oversize_refusal_keeps_the_exact_error() {
    let error = seal_definition(&ring(), &oversized(false)).unwrap_err();
    let ManagementError::Invalid(error) = error else {
        panic!("expected a size refusal")
    };
    assert_eq!(
        error,
        ValidationError::new("/", "sealed channel exceeds the record size limit")
    );
}

#[test]
fn extended_oversize_refusal_reports_the_checked_plaintext_bytes() {
    let definition = oversized(true);
    let bytes = canonical_json(definition.to_blob_json()).len();
    assert!(bytes > MAX_BLOB_BYTES);
    let error = seal_definition(&ring(), &definition).unwrap_err();
    let ManagementError::Invalid(error) = error else {
        panic!("expected a size refusal")
    };
    assert_eq!(
        error,
        ValidationError::new(
            "/",
            format!(
                "sealed channel exceeds the record size limit bytes={bytes} limit={MAX_BLOB_BYTES}"
            )
        )
    );
    let accepted = fixture_definition(routed());
    let blob = seal_definition(&ring(), &accepted).unwrap();
    assert_eq!(
        open_blob(&ring(), &blob).unwrap().payload,
        canonical_json(accepted.to_blob_json()).as_bytes()
    );
}

#[test]
fn management_routing_config_controls_only_the_routing_write_switch() {
    let providers = Arc::new(
        crate::providers::ProviderClient::new(
            crate::providers::ProviderRegistry::default(),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            1,
        )
        .unwrap(),
    );
    let mut state = ManagementState::new(
        providers,
        Arc::new(
            crate::records::RecordClient::new("http://127.0.0.1:8099", "fixture-service").unwrap(),
        ),
        Arc::new(crate::auth::CreatorAuthenticator::new("http://127.0.0.1:8098").unwrap()),
        crate::ManagementConfig {
            master_keys: Arc::new(ring()),
            public_url: "http://127.0.0.1:8090".into(),
            cma_base_url: "http://127.0.0.1:8099".into(),
            tools_nyxid_slug: "cmaeg".into(),
            cma_service_token: Secret::new("fixture-service"),
            nyxid_base_url: "http://127.0.0.1:8098".into(),
        },
        crate::ChannelCacheConfig::default(),
    );
    let next = fixture_definition(routed());
    for enabled in [false, true] {
        state.routing.multi_consumer_enabled = enabled;
        let flags = state.write_flags();
        assert_eq!(flags.multi_consumer, enabled);
        assert!(!flags.dynamic_sources);
        let result = write_gate::check(None, &next, flags);
        if enabled {
            result.unwrap();
        } else {
            assert_eq!(
                result,
                Err(ValidationError::new("/routing", "multi_consumer_disabled"))
            );
        }
    }
}
