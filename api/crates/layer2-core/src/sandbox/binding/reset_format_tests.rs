use super::*;
use crate::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use crate::SubjectId;
use serde_json::json;
const NOW: &str = "2026-09-22T00:00:00Z";

fn profile() -> BindingProfilePlan {
    let value = json!({
        "schema_version": 1,
        "metadata": {"name":"binding-test", "owner":{"kind":"human", "subject":"owner"}, "provider":"registered"},
        "model":{"access":"managed", "service":AgentProvider::default().connection_services().managed,
            "api_type":"supported", "model":"test", "thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"}, "context":{},
        "environment":{"size":{"cpu_cores":1, "memory_gib":2}, "workspace":{"type":"directory", "name":"project"}}
    });
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "supported",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap()
}

fn binding() -> ProviderBinding {
    ProviderBinding::new(
        "binding-one".into(),
        SubjectId::new("manager"),
        profile(),
        "a".repeat(64),
        NOW.into(),
    )
    .unwrap()
}

fn reset_state(binding: &ProviderBinding) -> ConversationResetState {
    let mut ready = binding.clone();
    ready.state = BindingState::Ready;
    ready.work = BindingWork::Idle;
    let successor = BindingConversation::new(&ready, "inc_successor".into(), NOW.into()).unwrap();
    ConversationResetState {
        heads: [(
            ConversationResetState::key(1, "transport"),
            ConversationHead {
                transport_conversation_id: "transport".into(),
                incarnation: 1,
                successor,
            },
        )]
        .into(),
        receipts: Default::default(),
    }
}

#[test]
fn format_is_omitted_until_promotion_and_unknown_formats_are_rejected() {
    let mut binding = binding();
    let legacy = serde_json::to_value(&binding).unwrap();
    assert!(legacy.get("reset_format").is_none());
    assert_eq!(
        serde_json::from_value::<ProviderBinding>(legacy).unwrap(),
        binding
    );
    binding.reset_format = Some(ConversationResetFormat::V1);
    let promoted = serde_json::to_value(&binding).unwrap();
    assert_eq!(promoted["reset_format"], "v1");
    assert_eq!(
        serde_json::from_value::<ProviderBinding>(promoted.clone()).unwrap(),
        binding
    );
    for unknown in [
        json!("v2"),
        json!(1),
        json!({"version":1}),
        json!([]),
        json!(null),
    ] {
        let mut invalid = promoted.clone();
        invalid["reset_format"] = unknown;
        assert!(serde_json::from_value::<ProviderBinding>(invalid).is_err());
    }
}

#[test]
fn marker_and_state_must_agree_and_ordinary_writes_cannot_change_the_floor() {
    let mut binding = binding();
    let state = reset_state(&binding);
    ConversationResetState::validate_format(&binding, None).unwrap();
    assert!(ConversationResetState::validate_format(&binding, Some(&state)).is_err());
    let mut next = binding.clone();
    next.advance_revision(NOW.into()).unwrap();
    next.reset_format = Some(ConversationResetFormat::V1);
    assert!(store::validate_binding_write(Some(&binding), Some(binding.revision), &next).is_err());
    binding.reset_format = Some(ConversationResetFormat::V1);
    ConversationResetState::validate_format(&binding, Some(&state)).unwrap();
    assert!(ConversationResetState::validate_format(&binding, None).is_err());
    assert!(ConversationResetState::validate_format(&binding, Some(&Default::default())).is_err());
    store::validate_binding_write(Some(&binding), Some(binding.revision), &next).unwrap();
    next.reset_format = None;
    assert!(store::validate_binding_write(Some(&binding), Some(binding.revision), &next).is_err());
    assert!(store::validate_binding_write(None, None, &binding).is_err());
}

#[test]
fn reset_state_checks_count_byte_and_lineage_bounds() {
    let binding = binding();
    let state = reset_state(&binding);
    state.validate(&binding.binding_id).unwrap();
    assert!(state.validate("another-binding").is_err());
    let head = state.heads.values().next().unwrap().clone();
    let mut too_many = state.clone();
    for i in 0..256 {
        too_many.heads.insert(i.to_string(), head.clone());
    }
    assert!(!too_many.within_capacity().unwrap());
    let mut too_large = state.clone();
    too_large
        .heads
        .values_mut()
        .next()
        .unwrap()
        .successor
        .binding_id = "x".repeat(4 * 1024 * 1024);
    assert!(!too_large.within_capacity().unwrap());
    assert!(too_large.validate(&binding.binding_id).is_err());
    let mut malformed = state;
    malformed.heads.values_mut().next().unwrap().incarnation = 0;
    assert!(malformed.validate(&binding.binding_id).is_err());
}
