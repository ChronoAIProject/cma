use super::*;
use crate::store::{
    agent_commands::read_command, bridge_definitions::decode_inbox, bridge_invokes::channel_source,
};
use layer2_core::bridge::ingress::StoredInbox;
use layer2_core::sandbox::agent_io::{AgentActor, AgentSourceKind, ChannelCommandSidecar};
use std::time::Duration;

pub(super) fn sidecar(
    row: &Document,
    allocation: &AllocationRecord,
) -> Result<ChannelCommandSidecar, Error> {
    let command = read_command(row).map_err(|_| PartitionCommandError::Corrupt)?;
    let envelope = &command.receipt.envelope;
    let sidecar: ChannelCommandSidecar = mongodb::bson::deserialize_from_bson(
        row.get("channel_admission")
            .cloned()
            .ok_or(PartitionCommandError::Corrupt)?,
    )
    .map_err(|_| PartitionCommandError::Corrupt)?;
    if sidecar.schema_version != 1
        || sidecar.allocation_id != allocation.id
        || sidecar.command_id != envelope.command_id
        || command.receipt.response_id.as_ref() != Some(&sidecar.response_id)
        || envelope.agent_id != allocation.placement.target().agent_id
        || envelope.owner != allocation.owner
        || envelope.source.kind != AgentSourceKind::Channel
        || envelope.source.reference != sidecar.inbox_id
        || envelope.correlation_id != sidecar.inbox_id
        || envelope.request_fingerprint != sidecar.request_fingerprint
    {
        return Err(PartitionCommandError::Corrupt.into());
    }
    Ok(sidecar)
}

pub(super) async fn command_source(
    database: &Database,
    row: &Document,
    sidecar: &ChannelCommandSidecar,
    session: &mut ClientSession,
) -> Result<Option<StoredInbox>, Error> {
    let archived =
        channel_source::archived(row, sidecar).map_err(|_| PartitionCommandError::Corrupt)?;
    let original = database
        .collection::<Document>("bridge_callback_inbox")
        .find_one(doc! {"_id":&sidecar.inbox_id})
        .session(&mut *session)
        .await?;
    let original = original.map(|row| {
        let correlation = doc! {"command_id":sidecar.command_id.as_str(),"response_id":sidecar.response_id.as_str(),"allocation_id":&sidecar.allocation_id,"body_sha256":&sidecar.body_sha256};
        if row.get_document("command_correlation").ok() != Some(&correlation) { return Err(PartitionCommandError::Corrupt); }
        decode_inbox(&row).map_err(|_| PartitionCommandError::Corrupt)
    }).transpose()?;
    if archived
        .as_ref()
        .zip(original.as_ref())
        .is_some_and(|(a, b)| a != b)
    {
        return Err(PartitionCommandError::Corrupt.into());
    }
    let source = archived.or_else(|| {
        original.filter(|inbox| {
            inbox.expires_at > mongodb::bson::DateTime::now().timestamp_millis() / 1000
        })
    });
    let Some(inbox) = source else { return Ok(None) };
    let command = read_command(row).map_err(|_| PartitionCommandError::Corrupt)?;
    let AgentActor::Channel {
        bridge_id,
        platform,
        chat_id,
        sender_id,
        ..
    } = &command.receipt.envelope.actor
    else {
        return Err(PartitionCommandError::Corrupt.into());
    };
    if inbox.id != sidecar.inbox_id
        || inbox.attestation.body_sha256 != sidecar.body_sha256
        || inbox.owner != command.receipt.envelope.owner
        || bridge_id != inbox.pin.bridge_id.as_str()
        || platform != &inbox.envelope.platform
        || chat_id != &inbox.envelope.chat_id
        || sender_id != &inbox.envelope.sender_id
    {
        return Err(PartitionCommandError::Corrupt.into());
    }
    Ok(inbox.capabilities.is_some().then_some(inbox))
}

pub(super) async fn first(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<Option<StoredInbox>, Error> {
    let mut commands = database.collection::<Document>("agent_commands")
        .find(doc! {"channel_admission.allocation_id":&allocation.id,"agent_id":allocation.placement.target().agent_id.as_str(),"channel_admission.inbox_id":allocation.first_source.inbox_id()})
        .limit(2).max_time(Duration::from_secs(2)).session(&mut *session).await?;
    let first = commands.next(&mut *session).await.transpose()?;
    if commands.next(&mut *session).await.transpose()?.is_some() {
        return Err(PartitionCommandError::Corrupt.into());
    }
    if let Some(row) = first {
        let sidecar = sidecar(&row, allocation)?;
        return command_source(database, &row, &sidecar, session).await;
    }
    let row = database
        .collection::<Document>("bridge_callback_inbox")
        .find_one(doc! {"_id":allocation.first_source.inbox_id()})
        .session(session)
        .await?;
    let inbox = row
        .map(|row| decode_inbox(&row).map_err(|_| PartitionCommandError::Corrupt))
        .transpose()?;
    Ok(inbox.filter(|inbox| {
        inbox.capabilities.is_some()
            && inbox.expires_at > mongodb::bson::DateTime::now().timestamp_millis() / 1000
    }))
}

pub(super) async fn original(
    database: &Database,
    row: &Document,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<StoredInbox, Error> {
    let records = majority_collection::<Document>(database, "bridge_allocations");
    let mut current = allocation.clone();
    let mut row = row.clone();
    for _ in 0..256 {
        if let Some(value) = row.get("partition_command_source") {
            let marker = value.as_document().ok_or(PartitionCommandError::Corrupt)?;
            if marker.len() != 2 || marker.get_i32("schema_version").ok() != Some(1) {
                return Err(PartitionCommandError::Corrupt.into());
            }
            let id = marker
                .get_str("receipt_id")
                .map_err(|_| PartitionCommandError::Corrupt)?;
            let document = majority_collection::<Document>(database, journal::COLLECTION)
                .find_one(doc! {"_id":id})
                .session(&mut *session)
                .await?
                .ok_or(PartitionCommandError::Corrupt)?;
            let journal = journal::decode(&document)?;
            let associated = if journal.result.id == current.id {
                &journal.result
            } else {
                journal
                    .successor
                    .as_ref()
                    .filter(|r| r.id == current.id)
                    .ok_or(PartitionCommandError::Corrupt)?
            };
            if associated.identity != current.identity
                || associated.owner != current.owner
                || associated.pin != current.pin
                || associated.config != current.config
                || associated.first_source != current.first_source
                || associated.placement != current.placement
            {
                return Err(PartitionCommandError::Corrupt.into());
            }
            return Ok(journal.source);
        }
        if let Some(source) = first(database, &current, session).await? {
            return Ok(source);
        }
        let Some(predecessor) = &current.predecessor else {
            return Err(PartitionCommandError::SourceUnavailable.into());
        };
        row = records
            .find_one(doc! {"_id":&predecessor.id})
            .session(&mut *session)
            .await?
            .ok_or(PartitionCommandError::Corrupt)?;
        let previous = decode_record(&row)?;
        if previous.id != predecessor.id
            || previous.identity.generation != predecessor.generation
            || previous.identity.bridge_id != current.identity.bridge_id
            || previous.identity.key != current.identity.key
            || previous.owner != current.owner
            || previous.pin != current.pin
            || previous.config != current.config
            || previous.first_source != current.first_source
            || previous.updated_at_ms != current.created_at_ms
            || previous.state != layer2_core::bridge::allocation::AllocationState::Retired
            || previous.lease.is_some()
        {
            return Err(PartitionCommandError::Corrupt.into());
        }
        current = previous;
    }
    Err(PartitionCommandError::Corrupt.into())
}
