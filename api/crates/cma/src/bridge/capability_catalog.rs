//! CMA contract fixtures at the pinned upstream revision; no live parity claim.
use crate::config::{bridge_flags::BridgeFlag, BridgeFeatureFlags};
use layer2_core::bridge::{partition::StableThreadKind, *};

pub(super) fn supported_platform(name: &str) -> Option<BridgePlatform> {
    match name {
        "telegram" => Some(BridgePlatform::Telegram),
        "telegram-new" => Some(BridgePlatform::TelegramNew),
        "discord" => Some(BridgePlatform::Discord),
        "lark" => Some(BridgePlatform::Lark),
        "feishu" => Some(BridgePlatform::Feishu),
        "slack" => Some(BridgePlatform::Slack),
        "whatsapp" => Some(BridgePlatform::Whatsapp),
        "x" => Some(BridgePlatform::X),
        _ => None,
    }
}

pub(super) fn catalog(connection_validation_available: bool) -> BridgeCapabilityCatalog {
    BridgeCapabilityCatalog {
        version: 1,
        source_revision: Some("85607255ee0beadca460a350e0aa6d177edd2fd2".into()),
        connection_validation_available,
        placements: shared_placements(None, BridgeFeatureFlags::default()),
        connectors: [
            "telegram",
            "telegram-new",
            "discord",
            "lark",
            "feishu",
            "slack",
            "whatsapp",
            "x",
            "device",
        ]
        .into_iter()
        .map(connector)
        .collect(),
    }
}

pub(super) fn shared_placements(
    runtime: Option<layer3_adapter::AdapterCapabilities>,
    flags: BridgeFeatureFlags,
) -> Vec<BridgePlacementCapabilities> {
    let supported = runtime.is_some_and(|caps| {
        caps.conversation
            && caps.per_agent_home
            && caps.steer
            && caps.interrupt
            && caps.usage
            && caps.resources
    });
    [
        (
            BridgeSharedPlacementKind::WorkspacePerPartition,
            BridgeFlag::WorkspacePerPartition,
        ),
        (
            BridgeSharedPlacementKind::WorktreeAgentPerPartition,
            BridgeFlag::WorktreeAgentPerPartition,
        ),
    ]
    .into_iter()
    .map(|(kind, flag)| BridgePlacementCapabilities {
        kind,
        availability: if !supported {
            BridgePlacementAvailability::RuntimeUnavailable
        } else if flags.enabled(flag) {
            BridgePlacementAvailability::Available
        } else {
            // Retain the published wire value for an unselected capable runtime.
            // Operator selection does not attest upgrade/rollback continuity.
            BridgePlacementAvailability::ContinuityUnverified
        },
    })
    .collect()
}

fn connector(name: &str) -> BridgeConnectorCapabilities {
    let boundary_tested = supported_platform(name).is_some();
    let implemented = matches!(
        name,
        "telegram" | "telegram-new" | "discord" | "lark" | "feishu"
    );
    BridgeConnectorCapabilities {
        platform: name.into(),
        evidence: if boundary_tested {
            BridgeCapabilityEvidence::ContractTested
        } else if name == "device" {
            BridgeCapabilityEvidence::IngestOnly
        } else {
            BridgeCapabilityEvidence::UpstreamOnly
        },
        live_verified: false,
        inbound_text: implemented,
        stable_sender_chat: boundary_tested,
        stable_thread: match name {
            "telegram" | "telegram-new" => Some(StableThreadKind::TelegramForumTopic),
            "discord" => Some(StableThreadKind::DiscordChannelThread),
            "lark" => Some(StableThreadKind::LarkThread),
            "feishu" => Some(StableThreadKind::FeishuThread),
            "slack" => Some(StableThreadKind::SlackThread),
            _ => None,
        },
        // Signed raw parent identity AND an exact permanent CMA Sent receipt.
        reply_to_bot: implemented,
        bot_mentions: false,
        inbound_edits: false,
        inbound_deletes: false,
        verified_actions: false,
        attachment_input: if implemented {
            BridgeAttachmentInput::DownloadRequired
        } else {
            BridgeAttachmentInput::Unavailable
        },
        attachment_output: false,
        output_text: implemented,
        // Explicit tool edits target only the original confirmed CMA reply.
        output_edit: matches!(name, "lark" | "feishu"),
        output_progress: implemented,
        output_limit_utf16: implemented.then_some(if matches!(name, "telegram" | "telegram-new") {
            4000
        } else {
            1900
        }),
        output_format: if !implemented {
            BridgeOutputFormat::Unavailable
        } else if matches!(name, "telegram" | "telegram-new" | "discord") {
            BridgeOutputFormat::EscapedMarkdown
        } else {
            BridgeOutputFormat::PlainText
        },
        stages: BridgeCapabilityStages::default(),
    }
}

#[cfg(test)]
mod tests;
