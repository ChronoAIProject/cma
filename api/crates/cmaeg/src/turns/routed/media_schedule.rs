//! Server-generated frozen scheduling hint. This is never accepted as event,
//! Agent, source, attachment or artifact authority from an incoming request.
use super::{ExecutionError, FrozenTurn};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    turns::render_media::{self, ItemNote, RenderMedia},
};
use cmaeg_core::{
    activity::{Activity, AttachmentKind},
    media_continuation::MediaContinuationKind,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub(super) const KEY: &str = "cmaeg_media_continuations_v1";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MediaSchedule {
    v: u8,
    t: Vec<u8>,
    x: Vec<u8>,
}
impl MediaSchedule {
    pub fn capture(activity: &Activity, media: &RenderMedia) -> Result<Self, ExecutionError> {
        let attachments = render_media::attachments(activity);
        if attachments.len() > 32 {
            return Err(ExecutionError::Invalid);
        }
        let mut value = Self {
            v: 1,
            t: Vec::new(),
            x: Vec::new(),
        };
        for (index, note) in &media.notes {
            if *note != ItemNote::Pending {
                continue;
            }
            let attachment = attachments.get(*index).ok_or(ExecutionError::Invalid)?;
            if attachment.source.is_none() {
                continue;
            }
            // Files are delivered as originals. Only transcripts currently
            // carry an exact consumed-source digest for safe late derivation.
            let list = match attachment.kind {
                AttachmentKind::Audio | AttachmentKind::Video => &mut value.t,
                _ => continue,
            };
            list.push(u8::try_from(*index).map_err(|_| ExecutionError::Invalid)?);
        }
        value.t.sort_unstable();
        value.t.dedup();
        value.x.sort_unstable();
        value.x.dedup();
        Ok(value)
    }
    pub fn encode(&self) -> Result<String, ExecutionError> {
        let value = serde_json::to_string(self).map_err(|_| ExecutionError::Invalid)?;
        if value.len() > 512 {
            return Err(ExecutionError::Invalid);
        }
        Ok(value)
    }
    pub fn read(
        body: &FrozenTurn,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<Self>, ExecutionError> {
        let Some(encoded) = body.metadata.get(KEY) else {
            return Ok(None);
        };
        if target.launch.provider.slug.as_str() != "cma_codex"
            || target.launch.provider.target.is_profile()
            || encoded.len() > 512
        {
            return Err(ExecutionError::Invalid);
        }
        let value: Self = serde_json::from_str(encoded).map_err(|_| ExecutionError::Corrupt)?;
        value.validate(plan, target)?;
        Ok(Some(value))
    }
    pub fn validate(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        if target.launch.provider.slug.as_str() != "cma_codex"
            || target.launch.provider.target.is_profile()
        {
            return Err(ExecutionError::Invalid);
        }
        let value = self;
        let attachments = render_media::attachments(&plan.activity);
        let mut seen = BTreeSet::new();
        if value.v != 1 || attachments.len() > 32 {
            return Err(ExecutionError::Corrupt);
        }
        for (index, transcript) in value
            .t
            .iter()
            .map(|v| (*v, true))
            .chain(value.x.iter().map(|v| (*v, false)))
        {
            let attachment = attachments
                .get(usize::from(index))
                .ok_or(ExecutionError::Corrupt)?;
            if !seen.insert(index)
                || attachment.source.is_none()
                || matches!(
                    attachment.kind,
                    AttachmentKind::Audio | AttachmentKind::Video
                ) != transcript
            {
                return Err(ExecutionError::Corrupt);
            }
        }
        Ok(())
    }
    pub fn derivative(&self, index: u8) -> Option<MediaContinuationKind> {
        if self.t.contains(&index) {
            Some(MediaContinuationKind::Transcript)
        } else if self.x.contains(&index) {
            Some(MediaContinuationKind::TextExcerpt)
        } else {
            None
        }
    }
}
