//! Read-only partial text, outside durable log and response event numbering.
use super::Agents;
use crate::{Error, Result, resources::identity};
use serde::Deserialize;

/// Typed discovery on a failed turn, never authority to read its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct RetainedOutputAvailability {
    /// True only for a private-prefix settlement; absent metadata means do not probe.
    pub available: bool,
}

/// Original immutable coordinates of a retained partial answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RetainedOutput {
    /// Opaque checkpoint identity, never an authorization token.
    pub id: String,
    /// Original Agent identity.
    pub agent_id: String,
    /// Original failed turn.
    pub turn_sequence: u64,
    /// Last committed log sequence when this prefix was retained.
    pub checkpoint_sequence: u64,
    /// Original capacity-failure event; not a new event.
    pub failure_sequence: u64,
    /// Original failure timestamp.
    pub failed_at: String,
    /// Exact UTF-8 byte length, at most 1 MiB.
    pub byte_length: usize,
    /// Always true; never treat this as a successful completion.
    pub partial: bool,
}
/// A UTF-8 chunk; concatenate content without separators and follow next.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RetainedOutputPage {
    /// None means no prefix is retained for this Agent or response turn.
    pub output: Option<RetainedOutput>,
    /// UTF-8 byte offset, independent of log sequences.
    pub offset: usize,
    /// At most 512 KiB of UTF-8 text.
    pub content: String,
    /// Opaque continuation bound to this checkpoint and Agent incarnation.
    pub next: Option<String>,
}
impl RetainedOutputPage {
    pub(crate) fn validate(&self, cursor: Option<&str>, agent: Option<&str>) -> Result<()> {
        if let Some(output) = &self.output {
            if let Some(agent) = agent {
                identity(&output.agent_id, agent)?;
            }
            let end = self
                .offset
                .checked_add(self.content.len())
                .ok_or_else(|| Error::protocol("retained output offset overflow"))?;
            if !output.partial
                || output.byte_length == 0
                || output.byte_length > 1024 * 1024
                || self.content.is_empty()
                || self.content.len() > 512 * 1024
                || end > output.byte_length
                || output.turn_sequence == 0
                || output.turn_sequence > output.checkpoint_sequence
                || output.checkpoint_sequence >= output.failure_sequence
                || self.next.is_some() != (end < output.byte_length)
                || (self.next.is_some() && self.next.as_deref() == cursor)
                || self.next.as_deref() == Some("")
                || (cursor.is_none() && self.offset != 0)
            {
                return Err(Error::protocol(
                    "retained output identity, bounds or continuation is inconsistent",
                ));
            }
        } else if self.offset != 0
            || !self.content.is_empty()
            || self.next.is_some()
            || cursor.is_some()
        {
            return Err(Error::protocol(
                "absent retained output has content or a continuation",
            ));
        }
        Ok(())
    }
}
impl Agents<'_> {
    /// Read exact retained text only after typed discovery. Do not change run outcomes or event cursors.
    pub async fn retained_output(
        &self,
        id: &str,
        cursor: Option<&str>,
        availability: Option<&crate::agents::RetainedOutputAvailability>,
    ) -> Result<RetainedOutputPage> {
        if cursor.is_none() && !availability.is_some_and(|value| value.available) {
            return Ok(RetainedOutputPage::default());
        }
        let query: Vec<_> = cursor
            .map(|value| ("cursor", value.to_owned()))
            .into_iter()
            .collect();
        let page: RetainedOutputPage = self
            .client
            .get(&["agents", id, "retained-output"], &query)
            .await?;
        page.validate(cursor, Some(id))?;
        Ok(page)
    }
}
