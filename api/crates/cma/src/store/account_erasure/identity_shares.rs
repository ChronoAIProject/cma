//! Legacy organization share evidence is retained without enabling new grants.
use super::{actor, has_subject, name, redact_actor_field};
use crate::store::{
    account_erasure::{references::remove_subjects, Error},
    resource_access::records,
};
use layer2_core::resource_access::{AgentShareAudience, AgentShareLink, ResourceOwner};
use mongodb::bson::{self, Bson, Document};
use std::collections::BTreeSet;

pub(super) fn replacement(
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<Document>, Error> {
    let name = name(collection)?;
    let paths = if collection == "resource_shares" {
        &[
            "record.issuer_actor.subject",
            "record.link.accepted_subjects",
            "record.link.audience.subjects",
            "accepted_subjects",
        ][..]
    } else {
        &[
            "record.actor.subject",
            "record.share.issuer_actor.subject",
            "record.share.link.accepted_subjects",
            "record.share.link.audience.subjects",
        ][..]
    };
    if !has_subject(row, paths, subjects) {
        return Ok(None);
    }
    let record = row
        .get_document("record")
        .map_err(|_| Error::InvalidRecord(name))?;
    let share = if collection == "resource_shares" {
        record
    } else {
        record
            .get_document("share")
            .map_err(|_| Error::InvalidRecord(name))?
    };
    let owner: ResourceOwner = bson::deserialize_from_bson(
        share
            .get("owner")
            .cloned()
            .ok_or(Error::InvalidRecord(name))?,
    )
    .map_err(|_| Error::InvalidRecord(name))?;
    if matches!(owner, ResourceOwner::Organization { .. }) {
        return organization(collection, row, subjects).map(Some);
    }
    if collection == "resource_shares" {
        return Ok(None);
    }
    let mut receipt = records::read_receipt(row).map_err(|_| Error::InvalidRecord(name))?;
    if !actor(&mut receipt.actor, subjects) {
        return Ok(None);
    }
    let at = receipt.at.clone();
    remove_subjects(&mut receipt.share, subjects, &at);
    let mut replacement = row.clone();
    replacement.extend(
        records::retained_receipt_document(&receipt, true)
            .map_err(|_| Error::InvalidRecord(name))?,
    );
    Ok(Some(replacement))
}

fn organization(
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Document, Error> {
    let name = name(collection)?;
    crate::store::sandbox_mongo::require_version(row).map_err(|_| Error::InvalidRecord(name))?;
    let mut replacement = row.clone();
    let record = replacement
        .get_document_mut("record")
        .map_err(|_| Error::InvalidRecord(name))?;
    let at = if collection == "resource_share_commands" {
        redact_actor_field(record, "actor", subjects, name)?;
        record.get_str("at").unwrap_or("").to_owned()
    } else {
        layer2_core::now_rfc3339()
    };
    let share = if collection == "resource_shares" {
        record
    } else {
        record
            .get_document_mut("share")
            .map_err(|_| Error::InvalidRecord(name))?
    };
    redact_actor_field(share, "issuer_actor", subjects, name)?;
    let accepted = link(share, subjects, &at, name)?;
    if collection == "resource_shares" {
        let revision = share
            .get_i64("revision")
            .ok()
            .filter(|n| *n > 0)
            .ok_or(Error::InvalidRecord(name))?;
        let next = revision.checked_add(1).ok_or(Error::InvalidRecord(name))?;
        share.insert("revision", next);
        let revoked = share
            .get("revoked_at")
            .is_some_and(|value| !matches!(value, Bson::Null));
        if row.get_i64("revision").ok() != Some(revision) {
            return Err(Error::InvalidRecord(name));
        }
        replacement.insert("revision", next);
        replacement.insert("revoked", revoked);
        if let Some(accepted) = accepted {
            replacement.insert("accepted_subjects", accepted);
        }
    }
    Ok(replacement)
}

fn link(
    share: &mut Document,
    subjects: &BTreeSet<String>,
    at: &str,
    name: &'static str,
) -> Result<Option<Vec<String>>, Error> {
    let Some(value) = share
        .get("link")
        .filter(|value| !matches!(value, Bson::Null))
    else {
        return Ok(None);
    };
    let mut link: AgentShareLink =
        bson::deserialize_from_bson(value.clone()).map_err(|_| Error::InvalidRecord(name))?;
    let revoked = share
        .get_str("revoked_at")
        .ok()
        .and_then(layer2_core::seconds_from_rfc3339)
        .is_some();
    let canonical_empty = revoked
        && link.accepted_subjects.is_empty()
        && matches!(&link.audience, AgentShareAudience::Users { subjects } if subjects.is_empty())
        && link.token_sha256.len() == 64
        && link
            .token_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !link.validate() && !canonical_empty {
        return Err(Error::InvalidRecord(name));
    }
    link.accepted_subjects
        .retain(|subject| !subjects.contains(subject.as_str()));
    if let AgentShareAudience::Users { subjects: audience } = &mut link.audience {
        audience.retain(|subject| !subjects.contains(subject.as_str()));
        if audience.is_empty() && !revoked {
            let created = share
                .get_str("created_at")
                .map_err(|_| Error::InvalidRecord(name))?;
            let effective = match (
                layer2_core::seconds_from_rfc3339(created),
                layer2_core::seconds_from_rfc3339(at),
            ) {
                (Some(created), Some(at_seconds)) if at_seconds >= created => at,
                (Some(_), _) => created,
                _ => return Err(Error::InvalidRecord(name)),
            };
            share.insert("revoked_at", effective.to_owned());
        }
    }
    let accepted = link
        .accepted_subjects
        .iter()
        .map(|subject| subject.as_str().to_owned())
        .collect();
    share.insert(
        "link",
        bson::serialize_to_bson(&link).map_err(|_| Error::InvalidRecord(name))?,
    );
    Ok(Some(accepted))
}
