use super::{configmap_value, CONFIGMAP};

#[test]
fn native_whatsapp_authoring_allows_owner_setup_while_new_media_stays_off() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED"),
        Some("true")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED"),
        Some("false")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED"),
        Some("false")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED"),
        Some("false")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED"),
        Some("false")
    );
}

#[test]
fn native_upload_effects_wait_for_compatible_reader_rollout() {
    for gate in [
        "CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED",
        "CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED",
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
        "CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED",
        "CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED",
        "CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED",
    ] {
        assert_eq!(configmap_value(CONFIGMAP, gate), Some("false"));
    }
}
