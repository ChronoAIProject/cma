//! Command monitoring proves the sign-up advisory reuses the spend snapshot.

use std::sync::{Arc, Mutex};

use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsState};
use layer2_core::SubjectId;
use mongodb::{
    bson::{doc, Bson, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};

use crate::auth::{AuthMode, AuthState};
use crate::store::{MongoRegistrationCapacityStore, MongoSandboxStore};

#[path = "callback_commands.rs"]
mod callback_commands;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unset_signup_capacity_reuses_the_existing_settings_read() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
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
    let name = format!(
        "cma_signup_capacity_commands_{}",
        uuid::Uuid::new_v4().simple()
    );
    let spend = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let capacity = Arc::new(
        MongoRegistrationCapacityStore::open(&client, &name, true)
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
        assert!(auth.registration_open().await);
        assert_eq!(
            *trace.lock().unwrap(),
            vec![("find".into(), "platform_settings".into())]
        );
    }
    let state = AdmissionCeilingsState::default()
        .changed(
            0,
            &SubjectId::new("operator"),
            "close registration capacity",
            AdmissionCeilings {
                registrations: Some(0),
                ..Default::default()
            },
            100,
        )
        .unwrap()
        .unwrap();
    settings
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$set":{"admission_ceilings":mongodb::bson::serialize_to_bson(&state).unwrap()}},
        )
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    assert!(!auth.registration_open().await);
    assert_eq!(
        *trace.lock().unwrap(),
        vec![
            ("find".into(), "platform_settings".into()),
            ("find".into(), "sandbox_admissions".into())
        ]
    );
    client.database(&name).drop().await.unwrap();
}
