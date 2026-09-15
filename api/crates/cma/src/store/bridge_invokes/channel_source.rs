//! Accepted Channel input survives callback TTL on its permanent command.
//! This sidecar contains normalized public message evidence, never the reply
//! token or protected thread-context ciphertext from callback reply custody.
use layer2_core::bridge::ingress::{InboxAdmission, StoredInbox};
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, ChannelCommandAdmissionError as Error, ChannelCommandSidecar,
};
use mongodb::{
    bson::{doc, Bson, Document},
    ClientSession,
};
use serde::{Deserialize, Serialize};

use super::{channel::MutationError, MongoBridgeInvokeStore};
use crate::store::{agent_commands::read_command, sandbox_mongo::record_bson};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedSource {
    schema_version: u16,
    inbox: StoredInbox,
    command_correlation: Document,
}

fn correlation(sidecar: &ChannelCommandSidecar) -> Document {
    doc! {
        "command_id": sidecar.command_id.as_str(),
        "response_id": sidecar.response_id.as_str(),
        "allocation_id": &sidecar.allocation_id,
        "body_sha256": &sidecar.body_sha256,
    }
}

pub(crate) fn archive(inbox: &StoredInbox, sidecar: &ChannelCommandSidecar) -> Result<Bson, Error> {
    validate(inbox, sidecar)?;
    record_bson(&AcceptedSource {
        schema_version: 1,
        inbox: inbox.clone(),
        command_correlation: correlation(sidecar),
    })
    .map_err(|_| Error::SourceMismatch)
}

fn validate(inbox: &StoredInbox, sidecar: &ChannelCommandSidecar) -> Result<(), Error> {
    if sidecar.schema_version != 1
        || inbox.admission != InboxAdmission::Accepted
        || inbox.id != sidecar.inbox_id
        || inbox.attestation.body_sha256 != sidecar.body_sha256
    {
        return Err(Error::SourceMismatch);
    }
    Ok(())
}

struct Snapshot {
    inbox: StoredInbox,
    original_present: bool,
}

async fn snapshot(
    invokes: &MongoBridgeInvokeStore,
    command: &Document,
    sidecar: &ChannelCommandSidecar,
    session: &mut ClientSession,
) -> Result<Snapshot, MutationError> {
    if command.get_str("_id").ok() != Some(sidecar.command_id.as_str()) {
        return Err(Error::SourceMismatch.into());
    }
    let archived = command
        .get("channel_source")
        .cloned()
        .map(|value| {
            let source: AcceptedSource =
                mongodb::bson::deserialize_from_bson(value).map_err(|_| Error::SourceMismatch)?;
            if source.schema_version != 1 || source.command_correlation != correlation(sidecar) {
                return Err(Error::SourceMismatch);
            }
            validate(&source.inbox, sidecar)?;
            Ok(source.inbox)
        })
        .transpose()?;
    let original = invokes
        .bridges
        .client
        .database(&invokes.bridges.definitions.namespace().db)
        .collection::<Document>("bridge_callback_inbox")
        .find_one(doc! {"_id": &sidecar.inbox_id})
        .session(session)
        .await?;
    let original_present = original.is_some();
    let original = original
        .map(|document| {
            let inbox = crate::store::bridge_definitions::decode_inbox(&document)
                .map_err(|_| Error::SourceMismatch)?;
            if document.get_document("command_correlation").ok() != Some(&correlation(sidecar)) {
                return Err(Error::SourceMismatch);
            }
            validate(&inbox, sidecar)?;
            Ok(inbox)
        })
        .transpose()?;
    let inbox = match (archived, original) {
        (Some(archived), Some(original)) if archived != original => {
            return Err(Error::SourceMismatch.into())
        }
        (Some(archived), _) => archived,
        // Old commands keep their original decode path until safely archived
        // by a normal, currently authorized admission replay.
        (None, Some(original)) => original,
        (None, None) => return Err(Error::SourceMismatch.into()),
    };
    Ok(Snapshot {
        inbox,
        original_present,
    })
}

pub(crate) async fn read(
    invokes: &MongoBridgeInvokeStore,
    command: &Document,
    sidecar: &ChannelCommandSidecar,
    session: &mut ClientSession,
) -> Result<StoredInbox, MutationError> {
    Ok(snapshot(invokes, command, sidecar, session).await?.inbox)
}

/// Shares execution/outbox transaction. A real write on the command protects
/// its permanent source; the original inbox is also fenced while it exists.
pub(crate) async fn fence(
    invokes: &MongoBridgeInvokeStore,
    command: &AdmittedAgentCommand,
    sidecar: &ChannelCommandSidecar,
    expected: &StoredInbox,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let document = invokes
        .resources
        .agent_commands
        .find_one(doc! {"_id": sidecar.command_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(Error::SourceMismatch)?;
    if read_command(&document).map_err(Error::from)? != *command {
        return Err(Error::SourceMismatch.into());
    }
    let current = snapshot(invokes, &document, sidecar, session).await?;
    if &current.inbox != expected {
        return Err(Error::SourceMismatch.into());
    }
    let changed = invokes
        .resources
        .agent_commands
        .update_one(
            doc! {"_id": sidecar.command_id.as_str(), "revision": command.revision as i64},
            doc! {"$inc": {"channel_source_fence": 1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(Error::SourceMismatch.into());
    }
    if current.original_present {
        let changed = invokes
            .bridges
            .client
            .database(&invokes.bridges.definitions.namespace().db)
            .collection::<Document>("bridge_callback_inbox")
            .update_one(
                doc! {"_id": &expected.id, "command_correlation": correlation(sidecar)},
                doc! {"$inc": {"channel_source_fence": 1_i64}},
            )
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
    }
    Ok(())
}
