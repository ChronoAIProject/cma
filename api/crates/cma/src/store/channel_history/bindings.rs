use super::*;

pub(crate) async fn partition_command(
    database: &Database,
    allocation: &AllocationRecord,
    journal: &Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    append(
        database,
        allocation,
        ReferenceKind::PartitionCommand,
        journal,
        session,
    )
    .await
}

pub(crate) async fn accepted_command(
    database: &Database,
    allocation: &AllocationRecord,
    command_id: &str,
    inbox_id: &str,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    for (kind, id) in [
        (ReferenceKind::AcceptedSource, command_id),
        (ReferenceKind::AcceptedCommand, command_id),
        (ReferenceKind::CallbackReceipt, inbox_id),
    ] {
        let rows: Collection<Document> = majority_collection(database, kind.collection());
        let document = rows
            .find_one(doc! {"_id": id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        append(database, allocation, kind, &document, session).await?;
    }
    Ok(())
}

pub(crate) async fn outbox(
    database: &Database,
    allocation_id: &str,
    items: &[layer2_core::bridge::outbox::OutboxItem],
    invocation: &Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let rows: Collection<Document> = majority_collection(database, "bridge_allocations");
    let document = rows
        .find_one(doc! {"_id": allocation_id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let allocation = crate::store::bridge_allocations::decode_record(&document)?;
    for item in items {
        let rows: Collection<Document> =
            majority_collection(database, ReferenceKind::OutboxItem.collection());
        let document = rows
            .find_one(doc! {"_id": &item.id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        if item.source.allocation_id() != allocation_id {
            return Err(AllocationError::IdentityMismatch.into());
        }
        append(
            database,
            &allocation,
            ReferenceKind::OutboxItem,
            &document,
            session,
        )
        .await?;
    }
    append(
        database,
        &allocation,
        ReferenceKind::OutboxInvocation,
        invocation,
        session,
    )
    .await
}

pub(crate) async fn tombstone(
    database: &Database,
    sandbox_id: &str,
    document: &Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let rows: Collection<Document> = majority_collection(database, "bridge_allocations");
    let mut cursor = rows
        .find(doc! {"creates_sandbox": true, "record.placement.target.sandbox_id": sandbox_id})
        .limit(2)
        .session(&mut *session)
        .await?;
    let mut allocation = None;
    while let Some(document) = cursor.next(&mut *session).await.transpose()? {
        if allocation.is_some() {
            return Err(AllocationError::IdentityMismatch.into());
        }
        allocation = Some(crate::store::bridge_allocations::decode_record(&document)?);
    }
    if let Some(allocation) = allocation {
        append(
            database,
            &allocation,
            ReferenceKind::LateUploadTombstone,
            document,
            session,
        )
        .await?;
    }
    Ok(())
}
