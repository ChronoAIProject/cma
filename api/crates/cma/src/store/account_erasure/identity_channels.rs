use super::{actor, creation, encoded, has_subject};
use crate::store::account_erasure::Error;
use crate::store::agent_channel_usage::records;
use layer2_core::sandbox::channel_usage::StoredAgentChannelUsage;
use mongodb::bson::{self, Document};
use std::collections::BTreeSet;

pub(super) fn replacement(
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<Document>, Error> {
    if collection == "agent_channel_usage" {
        const NAME: &str = "agent_channel_usage";
        if !has_subject(
            row,
            &[
                "record.state.creation.created_by.subject",
                "record.state.creation.creation_origin.launching_actor.subject",
            ],
            subjects,
        ) {
            return Ok(None);
        }
        let mut stored: StoredAgentChannelUsage =
            records::read_state(row).map_err(|_| Error::InvalidRecord(NAME))?;
        if !creation(&mut stored.state.creation, subjects) {
            return Ok(None);
        }
        stored
            .state
            .validate()
            .map_err(|_| Error::InvalidRecord(NAME))?;
        return encoded(row, &stored, NAME).map(Some);
    }
    const NAME: &str = "channel_usage_events";
    if !has_subject(
        row,
        &[
            "record.actor.subject",
            "record.before.creation.created_by.subject",
            "record.before.creation.creation_origin.launching_actor.subject",
            "record.after.creation.created_by.subject",
            "record.after.creation.creation_origin.launching_actor.subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut event = records::read_event(row).map_err(|_| Error::InvalidRecord(NAME))?;
    let original = event.digest();
    let mut changed = actor(&mut event.actor, subjects);
    changed |= creation(&mut event.before.creation, subjects);
    changed |= creation(&mut event.after.creation, subjects);
    if !changed {
        return Ok(None);
    }
    event.fingerprint = layer2_core::profiles::content_digest(&(
        "channel-usage-input-v1",
        &event.before.creation,
        event.before.revision,
        &event.actor,
        &event.change,
        event.reason,
    ))
    .map_err(|_| Error::InvalidRecord(NAME))?;
    let redacted =
        layer2_core::profiles::content_digest(&event).map_err(|_| Error::InvalidRecord(NAME))?;
    event
        .retain_erased_identity_digest(&original, &redacted)
        .map_err(|_| Error::InvalidRecord(NAME))?;
    let mut replacement = row.clone();
    replacement.extend(records::event_document(&event).map_err(|_| Error::InvalidRecord(NAME))?);
    replacement.insert(
        "record",
        bson::serialize_to_bson(&event).map_err(|_| Error::InvalidRecord(NAME))?,
    );
    replacement.insert("erased_identity", true);
    replacement.insert("redacted_digest", redacted);
    Ok(Some(replacement))
}
