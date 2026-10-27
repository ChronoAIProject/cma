use super::{configmap_value, CONFIGMAP};

#[test]
fn accepted_native_whatsapp_features_allow_owner_testing_with_explicit_gates() {
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
        Some("true")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED"),
        Some("true")
    );
    for gate in [
        "CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED",
        "CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED",
        "CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("true"), "{gate}");
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
fn native_lark_activated_intake_keeps_followup_media_and_effects_disabled() {
    for gate in [
        "CMAEG_NATIVE_LARK_WRITES_ENABLED",
        "CMAEG_NATIVE_LARK_INTAKE_ENABLED",
        "CMAEG_NATIVE_LARK_MEDIA_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("true"), "{gate}");
    }
    for gate in [
        "CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED",
        "CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED",
        "CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("false"));
    }
}
