//! New-format preparation and activation checks, compiled only on its writer.
use super::writer_rounds::{policy_snapshot, snapshot};
use super::*;
use crate::auth::registration::RegistrationCapacity;
use crate::store::writer_fixture::*;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsStore};
use serde_json::json;

async fn configure(client: &mongodb::Client, database: &str) {
    let store =
        crate::store::admission_ceilings::MongoAdmissionCeilingsStore::open(client, database, 0)
            .await
            .unwrap();
    for revision in 0..2 {
        store
            .replace(
                revision,
                &SubjectId::new("fixture-operator"),
                "synthetic compatibility policy",
                AdmissionCeilings {
                    retained_homes: Some(10 + revision as u32),
                    registrations: Some(100 + revision as u32),
                    ..Default::default()
                },
                100 + revision,
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn reserved_seed() {
    let _client = client().await;
    let case = input("CMA_WRITER_CASE");
    assert!(case.len() <= 20 && case.bytes().all(|b| b.is_ascii_alphanumeric()));
    let seed = seeded(&format!("writer_{case}")).await;
    crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
        .await
        .unwrap();
    crate::store::MongoRegistrationCapacityStore::open(&seed.client, &seed.database, true)
        .await
        .unwrap();
    configure(&seed.client, &seed.database).await;
    super::writer_rounds::admit_started(&seed).await;
    let observed = snapshot(&seed.client, &seed.database).await;
    assert_eq!(observed["marker"], true);
    assert_eq!(observed["pending_homes"], 1);
    assert_eq!(observed["reservation_state"], "reserved");
    emit(json!({"launch":observed,"retained":policy_snapshot(&seed.client,&seed.database).await}));
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn policy_seed() {
    let client = client().await;
    let database = database_name();
    configure(&client, &database).await;
    emit(json!({"database":database,"retained":policy_snapshot(&client,&database).await}));
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn activation_round() {
    let client = client().await;
    let database = database_name();
    let action = input("CMA_WRITER_ACTION");
    let admissions = client
        .database(&database)
        .collection::<Document>("sandbox_admissions");
    let key = doc! {"_id":{"scope":"registration_capacity"}};
    if action == "present" {
        crate::store::MongoRegistrationCapacityStore::open(&client, &database, true)
            .await
            .unwrap();
    } else if action == "malformed" {
        admissions
            .insert_one(doc! {"_id":{"scope":"registration_capacity"},
            "document_schema_version":1_i64,"activated_at":-1_i64,"count":0_i64})
            .await
            .unwrap();
    } else {
        assert_eq!(action, "absent");
    }
    let before = admissions.find_one(key.clone()).await.unwrap();
    let result =
        crate::store::MongoRegistrationCapacityStore::open(&client, &database, false).await;
    if action == "malformed" {
        assert!(
            result.is_err(),
            "invalid durable activation must refuse startup"
        );
        assert_eq!(before, admissions.find_one(key).await.unwrap());
        emit(json!({"action":action,"startup":"refused","marker_unchanged":true}));
        return;
    }
    let store = result.unwrap();
    assert!(store
        .admit(&SubjectId::new("fixture-new-registration"))
        .await
        .unwrap());
    let after = admissions.find_one(key).await.unwrap();
    let receipt_count = admissions
        .count_documents(doc! {"_id.scope":"registration"})
        .await
        .unwrap();
    if action == "absent" {
        assert!(before.is_none() && after.is_none());
        assert_eq!(receipt_count, 0);
    } else {
        assert_eq!(receipt_count, 1);
        assert_eq!(
            crate::store::registration_capacity::registration_count(after.as_ref().unwrap())
                .unwrap(),
            1
        );
    }
    emit(
        json!({"action":action,"write_flag":false,"startup":"accepted",
        "registration_receipts":receipt_count,"marker_before":hash(&before),"marker_after":hash(&after)}),
    );
}
