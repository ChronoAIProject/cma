//! Preserve image release order and serialize independent Desktop sizing selections.
use super::*;
use crate::store::{
    commit_transaction, is_confirmed_body_retry, start_transaction,
    transaction_terminal::GuardedTerminal as _, CommitOutcome,
};
use mongodb::ClientSession;
use std::time::Duration;

const ATTEMPTS: usize = 5;

enum SelectionError {
    Driver(mongodb::error::Error),
    Authority(WarmPoolError),
}

impl From<mongodb::error::Error> for SelectionError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<WarmPoolError> for SelectionError {
    fn from(error: WarmPoolError) -> Self {
        Self::Authority(error)
    }
}

pub(super) async fn activate(
    store: &MongoNativeWarmPools,
    candidate: &PoolTarget,
) -> Result<(), WarmPoolError> {
    if candidate.machine_plan_activated_at.is_some() {
        return Ok(());
    }
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(1 << attempt)).await;
        }
        let mut session = store
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_confirmed_body_retry(&error) {
                continue;
            }
            return Err(WarmPoolError::Unavailable);
        }
        match select_once(store, candidate, &mut session).await {
            Ok(false) => {
                session
                    .abort_guarded()
                    .await
                    .map_err(|_| WarmPoolError::Unavailable)?;
                return Ok(());
            }
            Ok(true) => (),
            Err(error) => {
                if let Err(abort) = session.abort_guarded().await {
                    tracing::warn!(%abort, "Desktop plan selection abort was not acknowledged");
                }
                match error {
                    SelectionError::Driver(error) if is_confirmed_body_retry(&error) => continue,
                    SelectionError::Driver(error) => {
                        tracing::warn!(%error, "Desktop plan selection transaction failed");
                        return Err(WarmPoolError::Unavailable);
                    }
                    SelectionError::Authority(error) => return Err(error),
                }
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Failed(error) => {
                tracing::warn!(%error, "Desktop plan selection commit failed");
                return Err(WarmPoolError::Unavailable);
            }
            CommitOutcome::Unknown(error) => {
                tracing::warn!(%error, "Desktop plan selection commit is uncertain; checking durable plan");
                // A committed selection is immutable. A different winning
                // candidate still fails the caller's latest-release check.
                let exact = store.find(&candidate.id).await?.is_some_and(|row| {
                    row.machine
                        && row.plan == candidate.plan
                        && !row.retiring
                        && !row.retired
                        && row.activated_at.is_some()
                        && row.machine_plan_activated_at.is_some()
                });
                return if exact {
                    Ok(())
                } else {
                    Err(WarmPoolError::Unavailable)
                };
            }
        }
    }
    Err(WarmPoolError::Unavailable)
}

/// Every Desktop selection and promotion writes the same deployment-scoped
/// document, even when no pool has ever been activated. The coordination
/// record is independent of target identity and survives their retirement.
pub(in crate::store::native_warm_pools) async fn selection_fence(
    store: &MongoNativeWarmPools,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    // Fixed-pin deployments can lack the optional released_runtimes grant.
    // Use a separate versioned settings record, leaving the platform singleton
    // and its operator-controlled revision independent of pool selections.
    let coordination =
        majority_collection::<Document>(&store.database, crate::store::settings::SETTINGS);
    let key = format!("machine_plan_selection:{}", store.deployment);
    coordination
        .update_one(
            doc! {"_id":key, "deployment":&store.deployment,
            "document_schema_version":crate::store::schema::DOCUMENT_SCHEMA_VERSION},
            doc! {"$inc":{"selection_fence":1_i64}},
        )
        .upsert(true)
        .session(&mut *session)
        .await?;
    Ok(())
}

async fn select_once(
    store: &MongoNativeWarmPools,
    candidate: &PoolTarget,
    session: &mut ClientSession,
) -> Result<bool, SelectionError> {
    selection_fence(store, session).await?;
    store
        .verify_activation_floor(true, &candidate.plan.spec.image.uri, session)
        .await?;

    let mut exact = release_scope(store, true);
    exact.extend(doc! {"_id":&candidate.id,"retiring":{"$ne":true},"retired":{"$ne":true}});
    let row = store
        .targets
        .find_one(exact.clone())
        .session(&mut *session)
        .await?
        .ok_or(WarmPoolError::Conflict)?;
    if row.machine_plan_activated_at.is_none() {
        let mut image_scope = release_scope(store, true);
        image_scope.extend(doc! {"plan.spec.image.uri": &row.plan.spec.image.uri,
        "activated_at": {"$type":"date"}});
        let mut selected = image_scope.clone();
        selected.insert("machine_plan_activated_at", doc! {"$type":"date"});
        let previously_selected = store
            .targets
            .find_one(selected)
            .sort(doc! {"machine_plan_activated_at": -1})
            .session(&mut *session)
            .await?
            .and_then(|row| row.machine_plan_activated_at);
        if row.activated_at.is_none() || previously_selected.is_none() {
            let previous = store
                .targets
                .find_one(image_scope)
                .sort(doc! {"activated_at":1})
                .session(&mut *session)
                .await?
                .and_then(|row| row.activated_at);
            let mut fields = doc! {"machine_plan_activated_at":"$$NOW"};
            if let Some(at) = previously_selected {
                let next = DateTime::from_millis(
                    at.timestamp_millis()
                        .checked_add(1)
                        .ok_or(WarmPoolError::Unavailable)?,
                );
                fields.insert("machine_plan_activated_at", doc! {"$max":["$$NOW",next]});
            }
            if row.activated_at.is_none() {
                match previous {
                    Some(at) => {
                        fields.insert("activated_at", at);
                    }
                    None => {
                        fields.insert("activated_at", "$$NOW");
                    }
                }
            }
            exact.insert("machine_plan_activated_at", mongodb::bson::Bson::Null);
            let written = store
                .targets
                .update_one(exact, vec![doc! {"$set":fields}])
                .session(&mut *session)
                .await?;
            if written.matched_count != 1 {
                return Err(WarmPoolError::Conflict.into());
            }
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an authenticated disposable replica set"]
    async fn fixed_pin_machine_selection_works_without_released_runtime_grant() {
        use mongodb::options::{ClientOptions, Credential};

        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_selection_{}", uuid::Uuid::new_v4().simple());
        let database = client.database(&name);
        let mut role = crate::store::isolation::role(&name);
        role.get_array_mut("privileges").unwrap().retain(|grant| {
            let collection = grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap();
            collection != crate::store::released_runtimes::COLLECTION
        });
        database.run_command(role).await.unwrap();
        let password = uuid::Uuid::new_v4().simple().to_string();
        database
            .run_command(doc! {"createUser":"fixed-pin-runtime","pwd":&password,
            "roles":[{"role":"cma_am_runtime","db":&name}]})
            .await
            .unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("fixed-pin-runtime".to_owned())
                .password(password)
                .source(name.clone())
                .build(),
        );
        let runtime = mongodb::Client::with_options(options)
            .unwrap()
            .database(&name);
        let denial = runtime
            .collection::<Document>(crate::store::released_runtimes::COLLECTION)
            .find_one(doc! {})
            .await
            .unwrap_err();
        assert!(
            matches!(denial.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 13)
        );

        let store = MongoNativeWarmPools::open(&runtime, "test").await.unwrap();
        let mut candidate = crate::warm_pools::tests::target();
        candidate.machine = true;
        candidate.document_schema_version = 2;
        assert!(crate::store::released_runtimes::StoreReleasedRuntime::open(
            &runtime,
            "test",
            layer3_adapter::ReleasedRuntime {
                revision: 0,
                agent: "registry.example/runtime@sha256:fixed".into(),
                machine: Some(candidate.plan.spec.image.uri.clone()),
                lane_revisions: None,
            },
            false
        )
        .await
        .unwrap()
        .is_none());
        store.insert(candidate.clone()).await.unwrap();
        store.activate(&candidate.id).await.unwrap();
        let selected = store.find(&candidate.id).await.unwrap().unwrap();
        assert!(selected.activated_at.is_some());
        assert!(selected.machine_plan_activated_at.is_some());
        crate::store::schema::verify_existing_collections(
            &runtime,
            &[crate::store::settings::SETTINGS],
        )
        .await
        .unwrap();
        database.drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn late_lower_id_target_cannot_bypass_the_stable_selection_fence() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let database = client.database(&format!("cma_selection_{}", uuid::Uuid::new_v4().simple()));
        let store = MongoNativeWarmPools::open(&database, "test").await.unwrap();
        let mut first = crate::warm_pools::tests::target();
        first.id = "z-pending".into();
        first.machine = true;
        first.document_schema_version = 2;
        store.insert(first.clone()).await.unwrap();
        let mut stale = client.start_session().await.unwrap();
        start_transaction(&mut stale).await.unwrap();
        assert!(store
            .targets
            .find_one(doc! {"_id": &first.id})
            .session(&mut stale)
            .await
            .unwrap()
            .is_some());

        let mut later = first.clone();
        later.id = "a-pending".into();
        later.plan.config_sha256 = "b".repeat(64);
        later.plan.spec.memory_mib *= 2;
        later.requested.memory_mib = later.plan.spec.memory_mib;
        store.insert(later.clone()).await.unwrap();
        store.activate(&later.id).await.unwrap();
        let winner = store.find(&later.id).await.unwrap().unwrap();
        match select_once(&store, &first, &mut stale).await {
            Err(SelectionError::Driver(error)) if is_confirmed_body_retry(&error) => {
                let _ = stale.abort_guarded().await;
            }
            Ok(true) => assert!(matches!(
                commit_transaction(&mut stale).await,
                CommitOutcome::Retry(_)
            )),
            _ => panic!("a stale snapshot must conflict on the stable fence"),
        }
        assert!(store
            .find(&first.id)
            .await
            .unwrap()
            .unwrap()
            .machine_plan_activated_at
            .is_none());
        store.activate(&first.id).await.unwrap();
        let retried = store.find(&first.id).await.unwrap().unwrap();
        assert_eq!(retried.activated_at, winner.activated_at);
        assert!(retried.machine_plan_activated_at > winner.machine_plan_activated_at);
        database.drop().await.unwrap();
    }
}
