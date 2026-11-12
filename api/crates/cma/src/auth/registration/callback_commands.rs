//! New subjects observe activation while known directory owners add no IO.

use super::*;
use crate::auth::{records::RegistrationDecision, SignInRecords};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, DirectoryQuery, Page,
    PublicIdentity, RegisteredUser, StoreError, UserDirectory, UserDirectoryStore,
};

struct Directory(mongodb::Collection<Document>);

#[async_trait::async_trait]
impl UserDirectoryStore for Directory {
    async fn observe(&self, _: &PublicIdentity) -> Result<(), StoreError> {
        unreachable!("admission does not update the directory")
    }

    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        let row = self
            .0
            .find_one(doc! {"_id":subject.as_str()})
            .await
            .map_err(|error| StoreError::Unavailable(error.to_string()))?;
        Ok(row.map(|_| RegisteredUser {
            identity: PublicIdentity {
                subject: subject.clone(),
                display_name: None,
                avatar_url: None,
            },
            first_seen_at: "2026-11-13T00:00:00Z".into(),
            last_seen_at: "2026-11-13T00:00:00Z".into(),
        }))
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        unreachable!("admission does not list the directory")
    }
}

#[async_trait::async_trait]
impl ActivityStore for Directory {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        unreachable!("admission does not append activity")
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!("admission does not list activity")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unactivated_unlimited_callback_checks_activation_once_without_writes() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if let Some(Bson::String(collection)) = event.command.get(&event.command_name) {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, collection.clone()));
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_reg_callback_cmd_{}", uuid::Uuid::new_v4().simple());
    let spend = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let capacity = Arc::new(
        MongoRegistrationCapacityStore::open(&client, &name, false)
            .await
            .unwrap(),
    );
    let directory = Arc::new(Directory(client.database(&name).collection("users")));
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "test-owner".into(),
        },
        Some(SignInRecords {
            users: UserDirectory::new(directory.clone()),
            activity: ActivityService::new(directory),
        }),
    )
    .with_platform_spend(spend)
    .with_registration_capacity(capacity);
    let settings = client
        .database(&name)
        .collection::<Document>("platform_settings");
    for explicit in [false, true] {
        let mutation = if explicit {
            doc! {"$set":{"admission_ceilings":mongodb::bson::serialize_to_bson(&AdmissionCeilingsState::default()).unwrap()}}
        } else {
            doc! {"$unset":{"admission_ceilings":""}}
        };
        settings
            .update_one(doc! {"_id":"platform"}, mutation)
            .await
            .unwrap();
        trace.lock().unwrap().clear();
        assert_eq!(
            auth.admit_registration(&SubjectId::new("new-public-subject"))
                .await
                .unwrap(),
            RegistrationDecision::Admitted
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec![
                ("find".into(), "users".into()),
                ("find".into(), "platform_settings".into()),
                ("find".into(), "sandbox_admissions".into())
            ]
        );
    }
    client
        .database(&name)
        .collection::<Document>("users")
        .insert_one(doc! {"_id":"known-directory-owner"})
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    assert_eq!(
        auth.admit_registration(&SubjectId::new("known-directory-owner"))
            .await
            .unwrap(),
        RegistrationDecision::Existing
    );
    assert_eq!(
        *trace.lock().unwrap(),
        vec![("find".into(), "users".into())]
    );
    client
        .database(&name)
        .collection::<Document>("auth_subjects")
        .insert_one(doc! {"_id":"retained-owner-without-directory"})
        .await
        .unwrap();
    settings
        .update_one(doc! {"_id":"platform"}, doc! {"$set":{"settings":false}})
        .await
        .unwrap();
    assert_eq!(
        auth.admit_registration(&SubjectId::new("retained-owner-without-directory"))
            .await
            .unwrap(),
        RegistrationDecision::Existing
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn a_running_callback_receiver_observes_later_activation() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_reg_late_activation_{}", uuid::Uuid::new_v4().simple());
    let spend = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let capacity = Arc::new(
        MongoRegistrationCapacityStore::open(&client, &name, false)
            .await
            .unwrap(),
    );
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "test-owner".into(),
        },
        None,
    )
    .with_platform_spend(spend)
    .with_registration_capacity(capacity);
    let before = SubjectId::new("signed-in-before-activation");
    assert_eq!(
        auth.admit_registration(&before).await.unwrap(),
        RegistrationDecision::Admitted
    );
    let admissions = client
        .database(&name)
        .collection::<Document>("sandbox_admissions");
    assert_eq!(admissions.count_documents(doc! {}).await.unwrap(), 0);
    client
        .database(&name)
        .collection::<Document>("auth_subjects")
        .insert_one(doc! {"_id":before.as_str()})
        .await
        .unwrap();
    MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    let after = SubjectId::new("new-subject-after-activation");
    assert_eq!(
        auth.admit_registration(&after).await.unwrap(),
        RegistrationDecision::Admitted
    );
    assert!(admissions
        .find_one(doc! {"_id":{"scope":"registration","subject":after.as_str()}})
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        auth.admit_registration(&after).await.unwrap(),
        RegistrationDecision::Existing
    );
    assert_eq!(
        auth.admit_registration(&before).await.unwrap(),
        RegistrationDecision::Existing
    );
    let counter = admissions
        .find_one(doc! {"_id":{"scope":"registration_capacity"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::store::registration_capacity::registration_count(&counter).unwrap(),
        1
    );
    assert!(admissions
        .find_one(doc! {"_id":{"scope":"registration","subject":before.as_str()}})
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}
