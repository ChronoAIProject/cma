//! Permanently refuse retained personal attribution until identity codecs ship.
use super::{references::Redaction, Error};
use layer2_core::account_deletion::ErasureBlock;
use mongodb::{bson::Document, Database};
use std::collections::BTreeSet;

pub(super) async fn redact(
    _db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    let paths: &[&str] = match collection {
        "resource_policies" => &["record.organization_evidence.admitted_by.subject"],
        "provider_bindings" => &["record.manager"],
        "agent_creation_provenance" => &[
            "record.created_by.subject",
            "record.creation_origin.launching_actor.subject",
        ],
        "agent_commands" => &["record.receipt.envelope.actor.subject"],
        "command_authorization_audits" => &[
            "record.actor.subject",
            "record.resources.actor.subject",
            "record.resources.observations.basis.subject",
        ],
        "resource_shares" => &[
            "record.issuer_actor.subject",
            "record.link.accepted_subjects",
            "record.link.audience.subjects",
            "accepted_subjects",
        ],
        "resource_share_commands" => &[
            "record.actor.subject",
            "record.share.issuer_actor.subject",
            "record.share.link.accepted_subjects",
            "record.share.link.audience.subjects",
        ],
        "sandbox_controls" => &[
            "record.envelope.actor.subject",
            "record.admission_audit.actor.subject",
            "record.execution_audit.actor.subject",
            "record.admission_audit.observations.basis.subject",
            "record.execution_audit.observations.basis.subject",
        ],
        "agent_artifacts" => &[
            "record.authorization.actor.subject",
            "record.authorization.observations.basis.subject",
        ],
        "bots" => &[
            "bot.ownership.created_by.subject",
            "bot.ownership.credential_subject",
        ],
        "activity" => &["record.actor", "record.detail.subject"],
        "agent_lifecycle_operations" => &["input.actor.subject", "owner"],
        "agent_channel_usage" => &[
            "record.state.creation.created_by.subject",
            "record.state.creation.creation_origin.launching_actor.subject",
        ],
        "channel_usage_events" => &[
            "record.actor.subject",
            "record.before.creation.created_by.subject",
            "record.before.creation.creation_origin.launching_actor.subject",
            "record.after.creation.created_by.subject",
            "record.after.creation.creation_origin.launching_actor.subject",
        ],
        _ => &[],
    };
    // This is an exact schema-field refusal, never a search of arbitrary text.
    // Keep all catalog identity scans active so unsupported residue cannot be
    // silently reported as complete. This adapter performs no storage writes.
    for path in paths {
        for subject in super::selectors::strings(row, path) {
            if subjects.contains(subject) {
                return Err(Error::blocked(
                    subject,
                    ErasureBlock::IdentityRedactionPending {
                        collection: collection.into(),
                    },
                ));
            }
        }
    }
    Ok(Redaction::Unchanged)
}
