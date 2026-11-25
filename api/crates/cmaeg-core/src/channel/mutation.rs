//! Non-expiring custody of an admitted channel management operation.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelMutation {
    pub nonce: String,
    pub operation_id: String,
    pub operation: ChannelMutationKind,
    pub original_version: u64,
    pub fingerprint: String,
    /// Separately sealed bounded target, including confirmed negotiated results.
    pub intended: String,
    pub completed: Vec<String>,
    /// Bounded exact step identity, independent of worker deadlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMutationKind {
    Create,
    Update,
    ProfileRevision,
    Delete,
    AuditRouting,
    AuditAttachments,
}

impl ChannelMutation {
    pub(super) fn validate(&self) -> Result<(), crate::ValidationError> {
        if self.original_version == 0
            || self.fingerprint.len() > 128
            || self.fingerprint.is_empty()
            || self.intended.is_empty()
            || self.intended.len() > 65536
            || self.completed.len() > 16
            || self
                .completed
                .iter()
                .any(|value| value.is_empty() || value.len() > 512)
            || self
                .completed
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.completed.len()
            || self
                .effect
                .as_ref()
                .is_some_and(|value| self.completed.contains(value))
        {
            return Err(crate::ValidationError::new(
                "/mutation",
                "invalid retained intent",
            ));
        }
        if self
            .effect
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 512)
        {
            return Err(crate::ValidationError::new(
                "/mutation/effect",
                "invalid effect identity",
            ));
        }
        if !uuid::Uuid::parse_str(&self.operation_id)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.operation_id)
        {
            return Err(crate::ValidationError::new(
                "/mutation/operation_id",
                "invalid operation identity",
            ));
        }
        if uuid::Uuid::parse_str(&self.nonce)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.nonce)
        {
            Ok(())
        } else {
            Err(crate::ValidationError::new(
                "/mutation/nonce",
                "invalid operation identity",
            ))
        }
    }
}
