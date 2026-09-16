use super::*;
use layer2_core::bridge::outbox::ChannelControlReferenceRecord;

/// The caller writes the immutable control effect and its reference together.
pub(crate) async fn control_record(
    database: &Database,
    reference: &ChannelControlReferenceRecord,
    kind: ReferenceKind,
    document: &Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    if !matches!(
        kind,
        ReferenceKind::ControlReference
            | ReferenceKind::ControlIntent
            | ReferenceKind::ControlCommand
            | ReferenceKind::ControlAlias
            | ReferenceKind::ControlDisposition
    ) {
        return Err(AllocationError::InvalidTransition.into());
    }
    reference.validate().map_err(|_| AllocationError::Corrupt)?;
    let row = majority_collection::<Document>(database, "bridge_allocations")
        .find_one(doc! {"_id": reference.binding().source.allocation_id()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let allocation = crate::store::bridge_allocations::decode_record(&row)?;
    reference
        .binding()
        .require_retained_allocation(&allocation)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    append(database, &allocation, kind, document, session).await
}
