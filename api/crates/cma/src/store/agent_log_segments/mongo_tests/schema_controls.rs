use super::*;
use segments::{
    publication::PublicationIdentity,
    writer_admission::{self, Admission, AdmissionKind},
    writer_gate::{self, GateName, GateRecord, Policy},
};

fn invalid(error: mongodb::error::Error) {
    assert!(matches!(*error.kind,
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(ref e))
        if e.code == 121));
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn additive_control_schema_accepts_typed_records_and_refuses_shape_drift() {
    let (client, name) = database("controls").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    for collection in [writer_gate::COLLECTION, writer_admission::COLLECTION] {
        assert!(COLLECTIONS.contains(&collection));
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    let current = agent();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let policy = Policy {
        enabled: true,
        agents: vec![current.id.0.clone()],
        ..Policy::default()
    };
    writer_gate::replace(&db, GateName::Segments, 0, policy.clone(), &mut session)
        .await
        .unwrap();
    writer_gate::replace(&db, GateName::CapacityRecovery, 0, policy, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
    let gates = db.collection::<Document>(writer_gate::COLLECTION);
    let row = gates
        .find_one(doc! {"_id":GateName::Segments.as_str()})
        .await
        .unwrap()
        .unwrap();
    GateRecord::read(GateName::Segments, row.clone()).unwrap();
    for case in 0..5 {
        let mut bad = row.clone();
        match case {
            0 => {
                bad.remove("slots");
            }
            1 => {
                bad.insert("revision", 1_i32);
            }
            2 => {
                bad.insert("unexpected", true);
            }
            3 => {
                bad.get_document_mut("policy")
                    .unwrap()
                    .insert("unexpected", true);
            }
            _ => {
                bad.insert("slots", vec![doc! {"holder":"fixture","fingerprint":digest::binary(vec![2;32]),"fence":1_i64,"expires_at":1_i64}]);
            }
        }
        invalid(
            gates
                .replace_one(doc! {"_id":GateName::Segments.as_str()}, bad)
                .await
                .unwrap_err(),
        );
        assert_eq!(
            gates
                .find_one(doc! {"_id":GateName::Segments.as_str()})
                .await
                .unwrap()
                .unwrap(),
            row
        );
    }
    let admissions = db.collection::<Document>(writer_admission::COLLECTION);
    for (number, kind) in [
        AdmissionKind::Activation,
        AdmissionKind::Rollover,
        AdmissionKind::CapacityRecovery,
    ]
    .into_iter()
    .enumerate()
    {
        let operation = PublicationIdentity {
            key: format!("fixture-operation-{number}"),
            fingerprint: digest::binary(vec![3; 32]),
            appended_at: NOW.into(),
        };
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let admission = writer_admission::admit(&db, &current, &operation, kind, NOW, &mut session)
            .await
            .unwrap();
        session.commit_transaction().await.unwrap();
        let stored = admissions
            .find_one(doc! {"_id":&admission.id})
            .await
            .unwrap()
            .unwrap();
        Admission::read(stored.clone()).unwrap();
        for case in 0..4 {
            let mut bad = stored.clone();
            match case {
                0 => {
                    bad.remove("recovery_gate");
                }
                1 => {
                    bad.insert("expected_seq", MAX_SEQUENCE + 1);
                }
                2 => {
                    bad.get_document_mut("writer_gate")
                        .unwrap()
                        .insert("revision", 1_i32);
                }
                _ => {
                    bad.insert("unexpected", true);
                }
            }
            invalid(
                admissions
                    .replace_one(doc! {"_id":&admission.id}, bad)
                    .await
                    .unwrap_err(),
            );
            assert_eq!(
                admissions
                    .find_one(doc! {"_id":&admission.id})
                    .await
                    .unwrap()
                    .unwrap(),
                stored
            );
        }
        let mut duplicate = stored;
        duplicate.insert("_id", format!("duplicate-{number}"));
        assert!(crate::store::sandbox_mongo::is_duplicate_key(
            &admissions.insert_one(duplicate).await.unwrap_err()
        ));
    }
    schema::verify(&db).await.unwrap();
    db.drop().await.unwrap();
}
