use super::*;
const MAX_REFERENCES: i64 = 1024;

pub(crate) async fn assess(
    database: &Database,
    allocation: &AllocationRecord,
    document: &Document,
    session: &mut ClientSession,
) -> Result<ChannelHistoryAssessment, MutationError> {
    let policy = policy(document, allocation)?;
    let mut result = ChannelHistoryAssessment {
        hold: policy.is_none().then_some(HistoryHold::LegacyPolicyMissing),
        policy,
        reference_count: 0,
        reference_sha256: None,
        inventory_sealed: false,
    };
    let count = match document.get_i64(COUNT) {
        Ok(count) if count >= 0 => count,
        _ => {
            result
                .hold
                .get_or_insert(HistoryHold::ReferenceAccountingMissing);
            return Ok(result);
        }
    };
    if count > MAX_REFERENCES {
        result.reference_count = count as u64;
        result
            .hold
            .get_or_insert(HistoryHold::InventoryBoundExceeded);
        return Ok(result);
    }
    let rows: Collection<Document> = majority_collection(database, REFERENCES);
    let mut cursor = rows
        .find(doc! {"allocation_id": &allocation.id})
        .sort(doc! {"_id": 1_i32})
        .limit(MAX_REFERENCES + 1)
        .session(&mut *session)
        .await?;
    let mut references = Vec::new();
    while let Some(document) = cursor.next(&mut *session).await.transpose()? {
        let reference = read_reference(&document)?;
        if reference.allocation_id != allocation.id {
            return Err(AllocationError::IdentityMismatch.into());
        }
        references.push(reference);
    }
    result.reference_count = references.len() as u64;
    if references.len() as i64 != count
        || !references
            .iter()
            .any(|r| r.kind == ReferenceKind::FirstSource && r.target_id == allocation.id)
    {
        result
            .hold
            .get_or_insert(HistoryHold::ReferenceAccountingIncomplete);
    }
    for reference in &references {
        let rows: Collection<Document> = majority_collection(database, reference.kind.collection());
        let Some(target) = rows
            .find_one(doc! {"_id": &reference.target_id})
            .session(&mut *session)
            .await?
        else {
            result
                .hold
                .get_or_insert(HistoryHold::ReferencedRecordMissing);
            continue;
        };
        if Reference::capture(allocation, reference.kind, &target)? != *reference {
            return Err(AllocationError::IdentityMismatch.into());
        }
    }
    result.reference_sha256 = Some(digest(&references)?);
    Ok(result)
}
