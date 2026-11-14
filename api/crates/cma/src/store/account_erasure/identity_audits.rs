//! Retained organization audit codecs preserve the current reader's projections.
use super::{actor, has_subject, resource_audit};
use crate::store::{
    account_erasure::Error, bots, resource_access::artifacts, sandbox_controls::records,
};
use layer2_core::{resource_access::ResourceOwner, SubjectId};
use mongodb::bson::Document;
use std::collections::BTreeSet;

pub(super) fn replacement(
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<Document>, Error> {
    match collection {
        "sandbox_controls" => control(row, subjects),
        "agent_artifacts" => artifact(row, subjects),
        "bots" => bot(row, subjects),
        _ => Ok(None),
    }
}

fn control(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "sandbox_controls";
    if !has_subject(
        row,
        &[
            "record.envelope.actor.subject",
            "record.admission_audit.actor.subject",
            "record.execution_audit.actor.subject",
            "record.admission_audit.observations.basis.subject",
            "record.execution_audit.observations.basis.subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record = records::read(row).map_err(|_| Error::InvalidRecord(NAME))?;
    let mut changed = actor(&mut record.envelope.actor, subjects);
    changed |= resource_audit(&mut record.admission_audit, subjects);
    if let Some(audit) = &mut record.execution_audit {
        changed |= resource_audit(audit, subjects);
    }
    if !changed {
        return Ok(None);
    }
    // This ledger uses fixed phase revisions (1 queued, 2 terminal). Its exact
    // record CAS and transaction write conflict fence protect attribution edits.
    let mut replacement = row.clone();
    replacement.extend(records::document(&record).map_err(|_| Error::InvalidRecord(NAME))?);
    Ok(Some(replacement))
}

fn artifact(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "agent_artifacts";
    if !has_subject(
        row,
        &[
            "record.authorization.actor.subject",
            "record.authorization.observations.basis.subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record = artifacts::read(row).map_err(|_| Error::InvalidRecord(NAME))?;
    if !resource_audit(&mut record.authorization, subjects) {
        return Ok(None);
    }
    let mut replacement = row.clone();
    replacement.extend(artifacts::document(&record).map_err(|_| Error::InvalidRecord(NAME))?);
    Ok(Some(replacement))
}

fn bot(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "bots";
    if !has_subject(
        row,
        &[
            "bot.ownership.created_by.subject",
            "bot.ownership.credential_subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record = bots::read_retained_bot(row).map_err(|_| Error::InvalidRecord(NAME))?;
    let Some(ownership) = &mut record.ownership else {
        return Ok(None);
    };
    if !matches!(ownership.owner, ResourceOwner::Organization { .. }) {
        return Ok(None);
    }
    let mut changed = actor(&mut ownership.created_by, subjects);
    if subjects.contains(ownership.credential_subject.as_str()) {
        ownership.credential_subject = SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT);
        ownership.credential_incarnation = None;
        ownership.authorization_grant_id = None;
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or(Error::InvalidRecord(NAME))?;
    let mut replacement = row.clone();
    replacement
        .extend(bots::retained_bot_document(&record).map_err(|_| Error::InvalidRecord(NAME))?);
    Ok(Some(replacement))
}
