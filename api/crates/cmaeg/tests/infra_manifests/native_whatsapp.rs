use super::{configmap_value, CONFIGMAP};

#[test]
fn verified_native_whatsapp_media_allows_owner_testing_while_next_features_wait() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED"),
        Some("true")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED"),
        Some("true")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED"),
        Some("true")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED"),
        Some("false")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED"),
        Some("false")
    );
    for gate in [
        "CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED",
        "CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED",
        "CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("false"), "{gate}");
    }
}

#[test]
fn native_upload_effects_follow_verified_compatible_reader_rollout() {
    for gate in [
        "CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED",
        "CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("true"), "{gate}");
    }
}

#[test]
fn received_reactions_wait_for_compatible_reader_rollout() {
    for gate in [
        "CMAEG_NATIVE_TELEGRAM_RECEIVED_REACTIONS_ENABLED",
        "CMAEG_NATIVE_DISCORD_RECEIVED_REACTIONS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("false"), "{gate}");
    }
}

#[test]
fn native_lark_requires_separate_explicit_authoring_and_intake_activation() {
    for gate in [
        "CMAEG_NATIVE_LARK_WRITES_ENABLED",
        "CMAEG_NATIVE_LARK_INTAKE_ENABLED",
        "CMAEG_NATIVE_LARK_MEDIA_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("false"));
    }
}
