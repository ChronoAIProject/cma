//! Read-only capacity checks may refuse, but never authorize a home.
use super::*;
use layer2_core::{admission::CapacityKind, SubjectId};

pub(super) enum Room {
    Available,
    Full,
    Unreadable,
}

pub(super) async fn check(
    admissions: &Collection<Document>,
    sandboxes: &Collection<Document>,
    mut limit: u32,
    reserve: u32,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<Room, mongodb::error::Error> {
    if reserve > 0
        && super::super::registration_capacity::is_public(admissions, owner, Some(session)).await?
    {
        limit = limit.saturating_sub(reserve);
    }
    if limit == 0 {
        return Ok(Room::Full);
    }
    let row = admissions
        .find_one(fence_key())
        .session(&mut *session)
        .await?;
    let pending = match pending_homes(row.as_ref()) {
        Ok(pending) => pending,
        Err(_) => return Ok(Room::Unreadable),
    };
    let available = u64::from(limit).saturating_sub(pending);
    if available == 0 {
        return Ok(Room::Full);
    }
    let live = sandboxes
        .count_documents(doc! {"retired_at": Bson::Null})
        .hint(mongodb::options::Hint::Name("sandboxes_global_live".into()))
        .limit(available)
        .max_time(Duration::from_secs(3))
        .session(session)
        .await?;
    Ok(if live >= available {
        Room::Full
    } else {
        Room::Available
    })
}

/// A retry that can already see a full cap need not contend on settings again.
/// None always continues through settings protection and authoritative admission.
pub(in crate::store) async fn precheck_fresh_home(
    settings: &Collection<Document>,
    admissions: &Collection<Document>,
    sandboxes: &Collection<Document>,
    deployment_limit: u16,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<Option<CapacityKind>, mongodb::error::Error> {
    let Some(document) = settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    if crate::store::settings::read_settings(document.clone()).is_err() {
        return Ok(None);
    }
    let Ok(policy) = crate::store::admission_ceilings::from_document(&document) else {
        return Ok(None);
    };
    let Some(limit) = policy.ceilings.effective_retained_homes(deployment_limit) else {
        return Ok(None);
    };
    match crate::store::registration_capacity::activation::read(admissions, Some(&mut *session))
        .await
    {
        Ok(true) => {}
        Ok(false) | Err(crate::store::registration_capacity::activation::Error::Unreadable(_)) => {
            return Ok(None)
        }
        Err(crate::store::registration_capacity::activation::Error::Driver(error)) => {
            return Err(error)
        }
    }
    let room = check(
        admissions,
        sandboxes,
        limit,
        policy.ceilings.existing_reserve.retained_homes.unwrap_or(0),
        owner,
        session,
    )
    .await?;
    Ok(matches!(room, Room::Full).then_some(CapacityKind::RetainedHomes))
}
