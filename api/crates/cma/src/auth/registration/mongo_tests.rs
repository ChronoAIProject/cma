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
    let settings = client
        .database(&name)
        .collection::<Document>("platform_settings");
    let defaults = AdmissionCeilingsState::default();
    let closed = defaults
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
    let cases = [
        (doc! {"$unset":{"admission_ceilings":""}}, true),
        (
            doc! {"$set":{"admission_ceilings":mongodb::bson::serialize_to_bson(&defaults).unwrap()}},
            true,
        ),
        (
            doc! {"$set":{"admission_ceilings":mongodb::bson::serialize_to_bson(&closed).unwrap()}},
            false,
        ),
    ];
    for activate in [false, true] {
        let capacity = Arc::new(
            MongoRegistrationCapacityStore::open(&client, &name, activate)
                .await
                .unwrap(),
        );
        let auth = AuthState::in_memory(
            AuthMode::Trusted {
                subject: "test-owner".into(),
            },
            None,
        )
        .with_platform_spend(spend.clone())
        .with_registration_capacity(capacity);
        for (mutation, open) in &cases {
            settings
                .update_one(doc! {"_id":"platform"}, mutation.clone())
                .await
                .unwrap();
            trace.lock().unwrap().clear();
            assert_eq!(auth.registration_open().await, *open);
            let mut reads = vec![("find".to_owned(), "platform_settings".to_owned())];
            if activate || !open {
                reads.push(("find".to_owned(), "sandbox_admissions".to_owned()));
            }
            assert_eq!(*trace.lock().unwrap(), reads);
        }
    }
    client.database(&name).drop().await.unwrap();
}
