//! Public availability facts, never a connection, admission or reply grant.
use super::partition::StableThreadKind;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BridgeCapabilityCatalog {
    pub version: u16,
    pub source_revision: Option<String>,
    pub connection_validation_available: bool,
    pub connectors: Vec<BridgeConnectorCapabilities>,
    /// Missing modes provide no placement-editing availability guarantee.
    pub placements: Vec<BridgePlacementCapabilities>,
}
impl Default for BridgeCapabilityCatalog {
    fn default() -> Self {
        Self {
            version: 1,
            source_revision: None,
            connection_validation_available: false,
            connectors: vec![],
            placements: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSharedPlacementKind {
    WorkspacePerPartition,
    WorktreeAgentPerPartition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgePlacementAvailability {
    Available,
    RuntimeUnavailable,
    ContinuityUnverified,
}

/// Deployment evidence for editing a placement. It is never target authority.
#[derive(Debug, Clone, Serialize)]
pub struct BridgePlacementCapabilities {
    pub kind: BridgeSharedPlacementKind,
    pub availability: BridgePlacementAvailability,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeCapabilityEvidence {
    ContractTested,
    UpstreamOnly,
    IngestOnly,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAttachmentInput {
    DownloadRequired,
    Unavailable,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeOutputFormat {
    EscapedMarkdown,
    PlainText,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeConnectorCapabilities {
    /// An upstream platform ID. Presence does not make it selectable in CMA.
    pub platform: String,
    pub evidence: BridgeCapabilityEvidence,
    pub live_verified: bool,
    pub inbound_text: bool,
    pub stable_sender_chat: bool,
    pub stable_thread: Option<StableThreadKind>,
    pub reply_to_bot: bool,
    pub bot_mentions: bool,
    pub inbound_edits: bool,
    pub inbound_deletes: bool,
    pub verified_actions: bool,
    pub attachment_input: BridgeAttachmentInput,
    pub attachment_output: bool,
    pub output_text: bool,
    pub output_edit: bool,
    /// Multiple explicit reply-tool messages during an eligible active turn.
    pub output_progress: bool,
    pub output_limit_utf16: Option<u32>,
    pub output_format: BridgeOutputFormat,
    /// Deployment switches only. Current owner/binding/consent are checked separately.
    pub stages: BridgeCapabilityStages,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct BridgeCapabilityStages {
    pub management: bool,
    pub ingress: bool,
    pub allocation: bool,
    pub execution: bool,
    pub outbound: bool,
}
