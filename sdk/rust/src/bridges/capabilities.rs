//! Public availability facts, never a connection, admission or reply grant.
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// Versioned connector capabilities observed by the CMA deployment.
pub struct BridgeCapabilityCatalog {
    /// Capability contract version; this SDK accepts version 1.
    pub version: u16,
    /// Exact upstream source revision used for the published contract evidence.
    pub source_revision: Option<String>,
    /// Whether this deployment can perform live connection validation.
    pub connection_validation_available: bool,
    /// Bounded connector inventory, including entries that cannot be selected.
    pub connectors: Vec<BridgeConnectorCapabilities>,
    /// Placement editing evidence. Absent legacy catalogs provide no availability.
    #[serde(default)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Shared-parent placement kind; separate conversations do not imply isolation.
pub enum BridgeSharedPlacementKind {
    /// Create a Workspace for each partition in the chosen Sandbox.
    WorkspacePerPartition,
    /// Create a Worktree and Agent in the chosen Workspace for each partition.
    WorktreeAgentPerPartition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Evidence for placement editing, independent of current target authorization.
pub enum BridgePlacementAvailability {
    /// Required runtime and continuity evidence is available; target checks still apply.
    Available,
    /// Required runtime abilities are absent or unknown.
    RuntimeUnavailable,
    /// Runtime abilities do not establish shared-parent upgrade/rollback continuity.
    ContinuityUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// One shared placement's evidence-backed availability.
pub struct BridgePlacementCapabilities {
    /// Exact supported placement kind.
    pub kind: BridgeSharedPlacementKind,
    /// Typed availability, never a resource or execution grant.
    pub availability: BridgePlacementAvailability,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// The evidence supporting this connector entry.
pub enum BridgeCapabilityEvidence {
    /// CMA contract fixtures verify the supported behavior.
    ContractTested,
    /// Only upstream source evidence is available; CMA stages remain unavailable.
    UpstreamOnly,
    /// Inbound transport is known, without a complete executable CMA contract.
    IngestOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Availability of attachment content at the inbound boundary.
pub enum BridgeAttachmentInput {
    /// Opaque attachment references need a separately authorized download.
    DownloadRequired,
    /// This capability is unavailable.
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Presentation format applied to explicit text replies.
pub enum BridgeOutputFormat {
    /// Text is escaped using the connector Markdown presentation rules.
    EscapedMarkdown,
    /// Text is sent using a plain text presentation contract.
    PlainText,
    /// This capability is unavailable.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// Supported connector behavior and independently enabled deployment stages.
pub struct BridgeConnectorCapabilities {
    /// An upstream platform ID. Presence does not make it selectable in CMA.
    pub platform: String,
    /// Strength of the fixture or upstream evidence behind this entry.
    pub evidence: BridgeCapabilityEvidence,
    /// Whether live platform receipts substantiate this contract.
    pub live_verified: bool,
    /// Whether original human text messages have a verified inbound contract.
    pub inbound_text: bool,
    /// Whether signed source fields establish exact sender and chat identities.
    pub stable_sender_chat: bool,
    /// Verified thread identity format, when available.
    pub stable_thread: Option<BridgeStableThread>,
    /// Whether confirmed Sent receipts can prove replies to the selected bot.
    pub reply_to_bot: bool,
    /// Whether a mention can be proven to address the selected bot.
    pub bot_mentions: bool,
    /// Whether inbound edits have a supported executable contract.
    pub inbound_edits: bool,
    /// Whether inbound deletes have a supported executable contract.
    pub inbound_deletes: bool,
    /// Whether interactive actions have a verified supported contract.
    pub verified_actions: bool,
    /// Whether attachments require a separately authorized download.
    pub attachment_input: BridgeAttachmentInput,
    /// Whether explicit reply tools can send attachments.
    pub attachment_output: bool,
    /// Whether eligible channel turns can send explicit text replies.
    pub output_text: bool,
    /// Whether explicit reply tools can edit an existing message.
    pub output_edit: bool,
    /// Multiple explicit reply-tool messages during an eligible active turn.
    pub output_progress: bool,
    /// Maximum UTF-16 code units per outbound text message.
    pub output_limit_utf16: Option<u32>,
    /// Escaping and presentation contract for outbound text.
    pub output_format: BridgeOutputFormat,
    /// Deployment switches only. Current owner/binding/consent are checked separately.
    pub stages: BridgeCapabilityStages,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
/// Independent deployment switches; none substitutes for live authority.
pub struct BridgeCapabilityStages {
    /// Whether this connector can perform new management effects.
    pub management: bool,
    /// Whether this connector can admit new inbound callbacks.
    pub ingress: bool,
    /// Whether this connector can allocate new channel resources.
    pub allocation: bool,
    /// Whether this connector can execute admitted channel commands.
    pub execution: bool,
    /// Whether this connector can send explicit outbound replies.
    pub outbound: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Platform field used as stable conversation thread identity.
pub enum BridgeStableThread {
    /// A Telegram or Telegram-new forum topic message thread ID.
    TelegramForumTopic,
    /// A verified Discord thread channel ID.
    DiscordChannelThread,
    /// A verified Lark thread ID.
    LarkThread,
    /// A verified Feishu thread ID.
    FeishuThread,
}

impl super::Bridges<'_> {
    /// Read capabilities under current owner/admin authority. Switches are not grants.
    pub async fn capabilities(
        &self,
        organization: Option<&str>,
    ) -> crate::Result<BridgeCapabilityCatalog> {
        let mut query = vec![];
        if let Some(org) = organization {
            super::validation::id(org)?;
            query.push(("organization", org.to_owned()));
        }
        let value: BridgeCapabilityCatalog = self
            .client
            .get(&["bridge-connectors", "capabilities"], &query)
            .await?;
        value.check()?;
        Ok(value)
    }
}
impl BridgeCapabilityCatalog {
    fn check(&self) -> crate::Result<()> {
        let mut platforms = std::collections::BTreeSet::new();
        let mut placements = std::collections::BTreeSet::new();
        if self.version != 1
            || self.connectors.len() > 32
            || self.placements.len() > 2
            || self.placements.iter().any(|p| !placements.insert(p.kind))
            || self.source_revision.as_ref().is_some_and(|s| {
                s.len() != 40
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            || self.connectors.iter().any(|c| {
                c.platform.is_empty()
                    || c.platform.len() > 64
                    || !c.platform.as_bytes()[0].is_ascii_lowercase()
                    || !c
                        .platform
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || !platforms.insert(&c.platform)
                    || c.output_limit_utf16 == Some(0)
                    || c.output_text != c.output_limit_utf16.is_some()
                    || c.output_text != (c.output_format != BridgeOutputFormat::Unavailable)
                    || ((c.output_edit || c.output_progress) && !c.output_text)
                    || (c.evidence != BridgeCapabilityEvidence::ContractTested
                        && (c.inbound_text
                            || c.output_text
                            || c.stages.management
                            || c.stages.ingress
                            || c.stages.allocation
                            || c.stages.execution
                            || c.stages.outbound))
            })
        {
            return Err(crate::Error::protocol(
                "bridge capabilities use an unknown or inconsistent contract",
            ));
        }
        Ok(())
    }
}
