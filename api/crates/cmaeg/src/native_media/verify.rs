//! A proved semantic edit can outlive eligibility for another edit reference.
use super::model::{Observation, Presentation, Snapshot};
use cmaeg_core::{
    message::{NativeMediaKind, NativeMediaUpdate},
    seal::native_media_edit::NativeMediaDestination as Destination,
};
use serde_json::Value;

pub(super) enum Confirmation {
    Renewable(Box<Snapshot>),
    Retired(&'static str),
}
pub(super) fn confirm(
    destination: &Destination,
    patch: &NativeMediaUpdate,
    before: &Snapshot,
    observed: Observation,
    intended: &Value,
) -> Option<Confirmation> {
    let after = observed.snapshot.for_revision(before)?;
    if after.kind != before.kind
        || Some(after.caption.as_str()) != intended["caption"].as_str()
        || Some(after.content.as_str()) != intended["content"].as_str()
        || !files_match(destination, patch, before, &after, intended)
    {
        return None;
    }
    let mut renewable = observed.eligible;
    match (&before.presentation, &after.presentation) {
        (None, None) => {}
        (
            Some(Presentation::Telegram {
                entities: old_entities,
                caption_above: old_above,
                spoiler: old_spoiler,
            }),
            Some(Presentation::Telegram {
                entities,
                caption_above,
                spoiler,
            }),
        ) => {
            if caption_above != old_above
                || spoiler != old_spoiler
                || (patch.caption.is_none() && entities != old_entities)
            {
                return None;
            }
        }
        (
            Some(Presentation::Discord {
                message_type: old_type,
                reply: old_reply,
                embeds: old_embeds,
                flags: old_flags,
                attachment_flags: old_attachment,
            }),
            Some(Presentation::Discord {
                message_type,
                reply,
                embeds,
                flags,
                attachment_flags,
            }),
        ) => {
            if message_type != old_type
                || reply != old_reply
                || flags != old_flags
                || (attachment_flags & 8) != (old_attachment & 8)
                || (patch.replacement.is_none() && attachment_flags != old_attachment)
            {
                return None;
            }
            // New content/assets can produce new previews. Alt-only writes do not
            // authorize rebasing a changed generated presentation into another edit.
            if patch.text.is_none() && patch.replacement.is_none() && embeds != old_embeds {
                renewable = false;
            }
        }
        _ => return None,
    }
    Some(if renewable {
        Confirmation::Renewable(Box::new(after))
    } else {
        Confirmation::Retired("native_media_optional_presentation_unavailable")
    })
}
fn files_match(
    destination: &Destination,
    patch: &NativeMediaUpdate,
    before: &Snapshot,
    after: &Snapshot,
    intended: &Value,
) -> bool {
    if patch.replacement.is_none() {
        return after.same_files(before);
    }
    match destination {
        Destination::Discord { .. } => {
            after.files.len() == 1
                && after.files[0].filename.as_deref() == intended["file"]["filename"].as_str()
                && after.files[0].size == intended["file"]["size"].as_u64()
                && after.files[0]
                    .mime
                    .as_deref()
                    .is_none_or(|mime| Some(mime) == intended["file"]["mime"].as_str())
                && !before.files.iter().any(|f| f.id == after.files[0].id)
        }
        Destination::Telegram { .. } => after.files.iter().all(|f| {
            f.filename
                .as_deref()
                .is_none_or(|v| Some(v) == intended["file"]["filename"].as_str())
                && f.mime
                    .as_deref()
                    .is_none_or(|v| Some(v) == intended["file"]["mime"].as_str())
                && (after.kind != NativeMediaKind::File
                    || f.size.is_none()
                    || f.size == intended["file"]["size"].as_u64())
        }),
    }
}
