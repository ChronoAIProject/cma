use super::*;
use crate::config::BridgeFeatureFlags;
use std::sync::Arc;

#[test]
fn upstream_only_and_device_connectors_never_gain_cma_admission_from_enabled_flags() {
    let flags = BridgeFeatureFlags::from_lookup(&mut |_| Ok(Some("true".into()))).unwrap();
    let configured = crate::bridge::feature_policy::configured(
        Arc::new(crate::bridge::UnavailableConnections),
        flags,
    );
    let catalog = configured.capabilities();
    assert!(!catalog.connection_validation_available);
    assert_eq!(catalog.connectors.len(), 9);
    for c in &catalog.connectors {
        assert!(!c.live_verified);
        assert!(!c.output_edit);
        assert!(!c.attachment_output);
        assert!(!c.bot_mentions);
        if ["telegram", "telegram-new", "discord", "lark", "feishu"].contains(&c.platform.as_str())
        {
            assert!(
                c.inbound_text
                    && c.stable_sender_chat
                    && c.reply_to_bot
                    && c.output_text
                    && c.output_progress
            );
            assert!(
                c.stages.management
                    && c.stages.ingress
                    && c.stages.allocation
                    && c.stages.execution
                    && c.stages.outbound
            );
        } else {
            assert!(!c.inbound_text && !c.output_text && c.stable_thread.is_none());
            assert!(
                !c.stages.management
                    && !c.stages.ingress
                    && !c.stages.allocation
                    && !c.stages.execution
                    && !c.stages.outbound
            );
        }
    }
    assert_eq!(
        supported_platform("telegram-new"),
        Some(BridgePlatform::TelegramNew)
    );
    assert!(supported_platform("Telegram").is_none());
}

#[test]
fn platform_and_stage_switches_are_projected_independently_without_provider_io() {
    let flags = BridgeFeatureFlags::from_lookup(&mut |key| {
        Ok(Some(
            matches!(
                key,
                "CHRONO_BRIDGE_TELEGRAM_ENABLED"
                    | "CHRONO_BRIDGE_INGRESS_ENABLED"
                    | "CHRONO_BRIDGE_OUTBOUND_ENABLED"
            )
            .to_string(),
        ))
    })
    .unwrap();
    let configured = crate::bridge::feature_policy::configured(
        Arc::new(crate::bridge::UnavailableConnections),
        flags,
    );
    for c in configured.capabilities().connectors {
        assert!(!c.stages.management && !c.stages.allocation && !c.stages.execution);
        assert_eq!(c.stages.ingress, c.platform == "telegram");
        assert_eq!(c.stages.outbound, c.platform == "telegram");
    }
}

#[test]
fn telegram_new_has_its_own_default_off_gate_and_telegram_presentation_limits() {
    for enabled_platform in [
        "CHRONO_BRIDGE_TELEGRAM_ENABLED",
        "CHRONO_BRIDGE_TELEGRAM_NEW_ENABLED",
    ] {
        let flags = BridgeFeatureFlags::from_lookup(&mut |key| {
            Ok(Some(
                (key == enabled_platform || key == "CHRONO_BRIDGE_INGRESS_ENABLED").to_string(),
            ))
        })
        .unwrap();
        let configured = crate::bridge::feature_policy::configured(
            Arc::new(crate::bridge::UnavailableConnections),
            flags,
        );
        let catalog = configured.capabilities();
        for c in catalog
            .connectors
            .iter()
            .filter(|c| c.platform.starts_with("telegram"))
        {
            assert_eq!(c.output_limit_utf16, Some(4000));
            assert_eq!(
                c.stages.ingress,
                (c.platform == "telegram")
                    == (enabled_platform == "CHRONO_BRIDGE_TELEGRAM_ENABLED")
            );
            assert!(!c.stages.outbound && !c.live_verified && !c.output_edit);
        }
    }
}
