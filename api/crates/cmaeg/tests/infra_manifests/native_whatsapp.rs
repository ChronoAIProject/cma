use super::{configmap_value, CONFIGMAP};

#[test]
fn native_whatsapp_authoring_stays_off_pending_reader_rollout_and_meta_acceptance() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED"),
        Some("false")
    );
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED"),
        Some("false")
    );
}
