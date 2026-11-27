//! Reserve retained homes when a fresh Launch is admitted, before resource work.

use std::time::Duration;

use layer2_core::admission::AdmissionCeilings;
use layer2_core::trigger_domain::{Launch, LaunchStoreError, ResolvedResource};
use layer2_core::StoreError;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{ClientSession, Collection};

use super::sandbox_mongo::require_version;
use super::schema::DOCUMENT_SCHEMA_VERSION;

mod room;
pub(in crate::store) use room::precheck_fresh_home;
pub(super) use room::{check as check_room, Room};

pub(in crate::store) enum Error {
    Domain(LaunchStoreError),
    Driver(mongodb::error::Error),
}

pub(in crate::store) fn receipt_key(launch: &str) -> Document {
    doc! {"_id": {"scope": "launch_home_capacity", "launch": launch}}
}

pub(in crate::store) fn fence_key() -> Document {
    doc! {"_id": {"scope": "global_sandbox_capacity"}}
}

pub(crate) fn pending_homes(document: Option<&Document>) -> Result<u64, StoreError> {
    let Some(document) = document else {
        return Ok(0);
    };
    require_version(document).map_err(StoreError::Malformed)?;
    match document.get("pending_launch_homes") {
        None => Ok(0),
        Some(Bson::Int64(count)) => u64::try_from(*count),
        Some(Bson::Int32(count)) => u64::try_from(*count),
        Some(_) => return Err(StoreError::Malformed("invalid pending home count".into())),
    }
    .map_err(|_| StoreError::Malformed("negative pending home count".into()))
}

pub(in crate::store) async fn reserve(
    admissions: &Collection<Document>,
    sandboxes: &Collection<Document>,
    deployment_limit: u16,
    ceilings: &Result<AdmissionCeilings, ()>,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<bool, Error> {
    if matches!(ceilings, Ok(policy) if policy.effective_retained_homes(deployment_limit).is_none())
    {
        return Ok(false);
    }
    let active = super::registration_capacity::activation::read(admissions, Some(&mut *session))
        .await
        .map_err(|error| match error {
            super::registration_capacity::activation::Error::Driver(error) => Error::Driver(error),
            super::registration_capacity::activation::Error::Unreadable(_) => refused(),
        })?;
    if !active {
        // An existing deployment hard cap does not authorize new durable shapes.
        // Pre-activation Launches keep #1672's worker-side admission authority.
        return Ok(false);
    }
    let ceilings = ceilings.as_ref().map_err(|_| refused())?;
    let Some(limit) = ceilings.effective_retained_homes(deployment_limit) else {
        return Ok(false);
    };
    let reused = matches!(launch.resolution.sandbox, ResolvedResource::Reused(_));
    if !reused {
        match room::check(
            admissions,
            sandboxes,
            limit,
            ceilings.existing_reserve.retained_homes.unwrap_or(0),
            &launch.owner,
            session,
        )
        .await
        .map_err(Error::Driver)?
        {
            room::Room::Available => {}
            room::Room::Full | room::Room::Unreadable => return Err(refused()),
        }
        // Only candidates with room write the shared fence. A concurrent
        // admission invalidates this transaction's older snapshot at that write.
        fence(admissions, session).await?;
        admissions
            .update_one(fence_key(), doc! {"$inc": {"pending_launch_homes": 1_i64}})
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
    }
    let mut receipt = receipt_key(launch.id.as_str());
    receipt.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    receipt.insert("owner", launch.owner.as_str());
    receipt.insert("state", if reused { "reused" } else { "reserved" });
    admissions
        .insert_one(receipt)
        .session(session)
        .await
        .map_err(Error::Driver)?;
    Ok(true)
}

pub(in crate::store) async fn release(
    admissions: &Collection<Document>,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<(), Error> {
    let reused = matches!(launch.resolution.sandbox, ResolvedResource::Reused(_));
    if !launch.state.is_terminal() && !reused {
        return Ok(());
    }
    let row = admissions
        .find_one(receipt_key(launch.id.as_str()))
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or_else(refused)?;
    require_version(&row).map_err(|_| refused())?;
    if row.get_str("owner").ok() != Some(launch.owner.as_str()) {
        return Err(refused());
    }
    let state = row.get_str("state").map_err(|_| refused())?;
    if !matches!(state, "reserved" | "reused" | "consumed" | "released") {
        return Err(refused());
    }
    if matches!(state, "consumed" | "released")
        || (state == "reused" && !launch.state.is_terminal())
    {
        return Ok(());
    }
    fence(admissions, session).await?;
    if state == "reserved" {
        let counter = admissions
            .find_one(fence_key())
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
        if pending_homes(counter.as_ref()).map_err(|_| refused())? == 0 {
            return Err(refused());
        }
        let mut filter = fence_key();
        filter.insert("pending_launch_homes", doc! {"$gte": 1_i64});
        let result = admissions
            .update_one(filter, doc! {"$inc": {"pending_launch_homes": -1_i64}})
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
        if result.matched_count != 1 {
            return Err(refused());
        }
    }
    let mut filter = receipt_key(launch.id.as_str());
    filter.insert("state", state);
    let result = admissions.update_one(filter,
        doc! {"$set": {"state": if launch.state.is_terminal() { "released" } else { "reused" }}})
        .session(session).await.map_err(Error::Driver)?;
    if result.matched_count != 1 {
        return Err(refused());
    }
    Ok(())
}

pub(in crate::store) async fn fence(
    admissions: &Collection<Document>,
    session: &mut ClientSession,
) -> Result<(), Error> {
    admissions
        .update_one(
            fence_key(),
            doc! {
                "$inc": {"fence": 1_i64},
                "$setOnInsert": {"document_schema_version": DOCUMENT_SCHEMA_VERSION},
            },
        )
        .upsert(true)
        .session(session)
        .await
        .map_err(Error::Driver)?;
    Ok(())
}

fn refused() -> Error {
    Error::Domain(LaunchStoreError::GlobalCapacityReached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_home_count_accepts_legacy_zero_and_rejects_corrupt_fences() {
        assert_eq!(pending_homes(None).unwrap(), 0);
        assert_eq!(
            pending_homes(Some(&doc! {"document_schema_version": 1_i64})).unwrap(),
            0
        );
        for count in [Bson::Int32(2), Bson::Int64(2)] {
            assert_eq!(
                pending_homes(Some(
                    &doc! {"document_schema_version": 1_i64, "pending_launch_homes": count}
                ))
                .unwrap(),
                2
            );
        }
        for row in [
            doc! {"document_schema_version": 2_i64, "pending_launch_homes": 1_i64},
            doc! {"document_schema_version": 1_i64, "pending_launch_homes": -1_i64},
            doc! {"document_schema_version": 1_i64, "pending_launch_homes": "2"},
        ] {
            assert!(matches!(
                pending_homes(Some(&row)),
                Err(StoreError::Malformed(_))
            ));
        }
    }
}
