use super::*;

pub(super) fn guard(
    filter: &mut Document,
    old: &AccountDeletion,
    next: &AccountDeletion,
    claim_at: Option<&str>,
) -> Result<(), AccountDeletionError> {
    let mut checks = Vec::new();
    if let Some(at) = claim_at {
        let lease = next
            .work
            .lease
            .as_ref()
            .ok_or(AccountDeletionError::Invalid)?;
        if lease.fence != old.work.next_fence {
            return Err(AccountDeletionError::StaleFence);
        }
        checks.push(doc! {"$lte":[records::date(&old.work.ready_at)?, "$$NOW"]});
        checks.push(doc! {"$lte":[records::date(at)?, "$$NOW"]});
        checks.push(doc! {"$lt":["$$NOW", records::date(&lease.expires_at)?]});
        if let Some(held) = &old.work.lease {
            checks.push(doc! {"$lte":[records::date(&held.expires_at)?, "$$NOW"]});
        }
    } else if let Some(held) = &old.work.lease {
        // Renewing or clearing a lease never extends the original admission window.
        checks.push(doc! {"$lt":["$$NOW", records::date(&held.expires_at)?]});
        if let Some(renewed) = &next.work.lease {
            checks.push(doc! {"$lt":["$$NOW", records::date(&renewed.expires_at)?]});
        }
    }
    if let Some(held) = &old.work.lease {
        filter.insert(
            "work.lease.fence",
            i64::try_from(held.fence).map_err(|_| AccountDeletionError::Invalid)?,
        );
    }
    if !checks.is_empty() {
        filter.insert("$expr", doc! {"$and":checks});
    }
    Ok(())
}
