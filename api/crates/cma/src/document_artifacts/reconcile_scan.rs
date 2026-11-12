use super::*;

pub(super) async fn step(
    store: &dyn ReconcileStore,
    blobs: Option<&dyn DocumentBlobMaintenance>,
    report: &mut ReconcileReport,
) -> Result<(), DocumentError> {
    match report.cursor.phase {
        0 => ledger_step(store, report).await,
        1 => reference_step(store, report).await,
        2 => listing_step(store, blobs, report).await,
        _ => {
            report.complete = true;
            Ok(())
        }
    }
}
async fn ledger_step(
    store: &dyn ReconcileStore,
    report: &mut ReconcileReport,
) -> Result<(), DocumentError> {
    let page = store
        .ledger_page(report.cursor.ledger_after.as_deref())
        .await?;
    if page.is_empty() {
        report.ledger_complete = true;
        report.age_scan_completed_at_ms = Some(mongodb::bson::DateTime::now().timestamp_millis());
        report.cursor.phase = 1;
        return Ok(());
    }
    if page.len() > PAGE {
        return Err(DocumentError::Corrupt);
    }
    for row in page {
        if report
            .cursor
            .ledger_after
            .as_ref()
            .is_some_and(|id| id >= &row.id)
        {
            return Err(DocumentError::Corrupt);
        }
        report.cursor.ledger_after = Some(row.id);
        if report.owner.as_ref().is_some_and(|v| v != &row.owner) {
            continue;
        }
        let owner = row.owner;
        let totals = report.owners.entry(owner.clone()).or_default();
        match row.kind.as_str() {
            "owner" => {
                totals.used = row.used;
                totals.baseline = row.baseline;
                if row
                    .baseline
                    .is_some_and(|v| v[0] > OWNER_DOCUMENT_LIMIT || v[1] > OWNER_DOCUMENT_BYTES)
                {
                    report.baseline_overshoot_owners.push(owner);
                }
            }
            "document" => totals.rows[0] = add(totals.rows[0], 1)?,
            "object" => {
                parse_key(row.object_key.as_deref().ok_or(DocumentError::Corrupt)?)?;
                totals.rows[1] = add(totals.rows[1], row.bytes)?;
                totals.rows[2] = add(totals.rows[2], 1)?;
                let threshold = match row.state.as_deref().unwrap_or("reserved") {
                    "reserved" => Some(RECLAIM_GRACE + RECLAIM_INTERVAL),
                    "condemned" => Some(RECLAIM_INTERVAL),
                    "committed" => None,
                    _ => return Err(DocumentError::Corrupt),
                };
                if let Some(threshold) = threshold {
                    if let Some(since) = row.since_ms {
                        if since > report.observed_at_ms {
                            report.unknown_age_rows += 1;
                        } else if report.observed_at_ms.saturating_sub(since)
                            > threshold.as_millis() as i64
                        {
                            report.aged_rows += 1;
                        }
                    } else {
                        report.unknown_age_rows += 1;
                    }
                }
            }
            _ => return Err(DocumentError::Corrupt),
        }
    }
    Ok(())
}
async fn reference_step(
    store: &dyn ReconcileStore,
    report: &mut ReconcileReport,
) -> Result<(), DocumentError> {
    if report.cursor.reference.is_none() {
        let Some(reference) = store
            .reference_page(report.cursor.reference_after.as_deref())
            .await?
        else {
            report.references_complete = true;
            report.cursor.phase = 2;
            return Ok(());
        };
        if report
            .cursor
            .reference_after
            .as_ref()
            .is_some_and(|v| v >= &reference.id)
        {
            return Err(DocumentError::Corrupt);
        }
        if report.owner.as_ref().is_some_and(|v| v != &reference.owner) {
            report.cursor.reference_after = Some(reference.id);
            return Ok(());
        }
        report.owners.entry(reference.owner.clone()).or_default();
        report.cursor.reference = Some(reference);
        report.cursor.reference_index = 0;
    }
    let reference = report
        .cursor
        .reference
        .as_ref()
        .ok_or(DocumentError::Corrupt)?;
    if let Some(key) = reference.keys.get(report.cursor.reference_index) {
        if parse_key(key)?.owner_hash != reference.owner {
            return Err(DocumentError::Corrupt);
        }
        if store.object_state(key).await?.as_deref() != Some("committed") {
            report.missing_committed += 1;
        }
        report.cursor.reference_index += 1;
    } else {
        report.cursor.reference_after = Some(reference.id.clone());
        report.cursor.reference = None;
    }
    Ok(())
}
async fn listing_step(
    store: &dyn ReconcileStore,
    blobs: Option<&dyn DocumentBlobMaintenance>,
    report: &mut ReconcileReport,
) -> Result<(), DocumentError> {
    if !report.listing_supported {
        report.cursor.phase = 3;
        return Ok(());
    }
    let blobs = blobs.ok_or(DocumentError::Unavailable)?;
    if report.cursor.listing_owner.is_none() {
        report.cursor.listing_owner = report
            .owner
            .clone()
            .or_else(|| report.owners.keys().next().cloned());
    }
    let Some(owner) = report.cursor.listing_owner.clone() else {
        report.listing_complete = true;
        report.cursor.phase = 3;
        return Ok(());
    };
    let prefix = format!("documents/{owner}/");
    if report.cursor.listing_pending.is_none() {
        let page = blobs
            .list(&prefix, report.cursor.listing_cursor.as_deref())
            .await
            .map_err(|e| e.error)?;
        if page.keys.len() > MAX_BLOB_PAGE_KEYS
            || page.next.as_ref().is_some_and(|next| {
                next.len() > MAX_BLOB_CURSOR_BYTES
                    || Some(next) == report.cursor.listing_cursor.as_ref()
            })
        {
            return Err(DocumentError::Corrupt);
        }
        report.cursor.listing_pending = Some(BlobKeys {
            keys: page.keys,
            next: page.next,
        });
        report.cursor.listing_index = 0;
    }
    let page = report
        .cursor
        .listing_pending
        .as_ref()
        .ok_or(DocumentError::Corrupt)?;
    if let Some(key) = page.keys.get(report.cursor.listing_index) {
        if report.cursor.listing_seen_keys.contains(key) {
            return Err(DocumentError::Corrupt);
        }
        if !key.starts_with(&prefix) || parse_key(key)?.owner_hash != owner {
            return Err(DocumentError::Corrupt);
        }
        if store.object_state(key).await?.is_none() {
            report.known_orphan_blob_count += 1;
            if !store.referenced(key).await? {
                report.orphans.push(key.clone());
            }
        }
        report.cursor.listing_seen_keys.insert(key.clone());
        report.cursor.listing_index += 1;
    } else {
        let next = page.next.clone();
        if next
            .as_ref()
            .is_some_and(|next| !report.cursor.listing_seen_cursors.insert(next.clone()))
        {
            return Err(DocumentError::Corrupt);
        }
        report.cursor.listing_pending = None;
        report.cursor.listing_cursor = next;
        if report.cursor.listing_cursor.is_none() {
            report.cursor.listing_seen_keys.clear();
            report.cursor.listing_seen_cursors.clear();
            report.cursor.listing_owner = if report.owner.is_some() {
                None
            } else {
                report
                    .owners
                    .range((std::ops::Bound::Excluded(owner), std::ops::Bound::Unbounded))
                    .next()
                    .map(|(owner, _)| owner.clone())
            };
            if report.cursor.listing_owner.is_none() {
                report.listing_complete = true;
                report.cursor.phase = 3;
            }
        }
    }
    Ok(())
}
fn add(left: u64, right: u64) -> Result<u64, DocumentError> {
    left.checked_add(right).ok_or(DocumentError::Corrupt)
}
