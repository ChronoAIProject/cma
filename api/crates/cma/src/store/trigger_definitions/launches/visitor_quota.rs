//! Daily visitor quota commits with Launch admission, including replay recovery.
use super::{admission::AttemptError, LaunchCollections};
use layer2_core::{
    trigger_domain::{visitor_launch_day, Launch, LaunchAuthorityClass, LaunchStoreError},
    StoreError, SubjectId,
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{IndexOptions, ReadPreference, SelectionCriteria},
    ClientSession, IndexModel,
};
use std::time::{Duration, UNIX_EPOCH};

pub(super) const COLLECTION: &str = "visitor_launch_days";

pub(super) async fn ensure_indexes(data: &LaunchCollections) -> Result<(), StoreError> {
    data.visitor_days
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expires_at": 1 })
                .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                .build(),
        )
        .await
        .map_err(|error| StoreError::Unavailable(format!("visitor quota index: {error}")))?;
    Ok(())
}

pub(super) async fn admitted(
    data: &LaunchCollections,
    owner: &SubjectId,
    day: u64,
) -> Result<u32, LaunchStoreError> {
    let Some(row) = data
        .visitor_days
        .find_one(doc! { "_id": format!("{day}:{}", owner.as_str()) })
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .await
        .map_err(super::unavailable)?
    else {
        return Ok(0);
    };
    crate::store::sandbox_mongo::require_version(&row).map_err(super::corrupt)?;
    let used = row
        .get_i64("used")
        .map_err(|error| super::corrupt(error.to_string()))?;
    u32::try_from(used).map_err(|error| super::corrupt(error.to_string()))
}

pub(super) struct Charge {
    key: String,
    document: Document,
    exists: bool,
}

/// A refusal-only optimization; a successful precheck is never a charge.
pub(super) async fn precheck(
    data: &LaunchCollections,
    launch: &Launch,
    initial: crate::store::settings_defaults::InitialSettings,
    session: &mut ClientSession,
) -> Result<(), AttemptError> {
    use AttemptError::{Domain, Driver};
    if launch.authority != LaunchAuthorityClass::Visitor {
        return Ok(());
    }
    let settings = data
        .settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(Driver)?;
    let settings = match settings {
        Some(document) => {
            crate::store::settings::read_settings(document)
                .map_err(|error| Domain(LaunchStoreError::Corrupt(error.to_string())))?
                .settings
        }
        None => initial.platform(),
    };
    prepare_with_limit(
        data,
        launch,
        settings.limits.visitor_max_launches_per_day,
        session,
    )
    .await?;
    Ok(())
}

pub(super) async fn prepare_with_limit(
    data: &LaunchCollections,
    launch: &Launch,
    limit: u32,
    session: &mut ClientSession,
) -> Result<Option<Charge>, AttemptError> {
    use AttemptError::{Domain, Driver};
    if launch.authority != LaunchAuthorityClass::Visitor {
        return Ok(None);
    }
    let timestamp = layer2_core::seconds_from_rfc3339(&launch.created_at)
        .ok_or_else(|| Domain(LaunchStoreError::Corrupt("invalid Launch time".into())))?;
    let (day, reset_at) = visitor_launch_day(timestamp);
    let start = UNIX_EPOCH + Duration::from_secs(day * 86_400);
    let key = format!("{day}:{}", launch.owner.as_str());
    let previous = data
        .visitor_days
        .find_one(doc! { "_id": &key })
        .session(&mut *session)
        .await
        .map_err(Driver)?;
    // Seed existing admissions during the first rollout; no quota reset on restart.
    let existing = data.launches.count_documents(doc! {
        "owner": launch.owner.as_str(), "record.authority": "visitor",
        "created_at": { "$gte": layer2_core::rfc3339_at(start), "$lt": layer2_core::rfc3339_at(reset_at) },
    }).session(&mut *session).await.map_err(Driver)?;
    let used = match previous.as_ref() {
        Some(row) => u64::try_from(
            row.get_i64("used")
                .map_err(|error| Domain(LaunchStoreError::Corrupt(error.to_string())))?,
        )
        .map_err(|error| Domain(LaunchStoreError::Corrupt(error.to_string())))?
        .max(existing),
        None => existing,
    };
    if used >= u64::from(limit) {
        return Err(Domain(LaunchStoreError::VisitorQuota { limit, reset_at }));
    }
    let document = doc! { "_id": &key, "document_schema_version": 1_i64, "used": (used + 1) as i64,
    "expires_at": DateTime::from_system_time(reset_at + Duration::from_secs(86_400)) };
    Ok(Some(Charge {
        key,
        document,
        exists: previous.is_some(),
    }))
}

pub(super) async fn charge(
    data: &LaunchCollections,
    charge: Option<Charge>,
    session: &mut ClientSession,
) -> Result<(), AttemptError> {
    let Some(charge) = charge else {
        return Ok(());
    };
    if charge.exists {
        data.visitor_days
            .replace_one(doc! { "_id": charge.key }, charge.document)
            .session(&mut *session)
            .await
            .map_err(AttemptError::Driver)?;
    } else {
        data.visitor_days
            .insert_one(charge.document)
            .session(&mut *session)
            .await
            .map_err(AttemptError::Driver)?;
    }
    Ok(())
}
