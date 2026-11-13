//! Qualify the frozen storage shape; actual codec binding is a separate gate.
use super::recovery_shape_fixture::carrier;
use super::*;

fn invalid(error: mongodb::error::Error) {
    assert!(matches!(*error.kind,
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(ref e))
        if e.code == 121));
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn recovery_schema_accepts_frozen_phases_and_refuses_invalid_shapes() {
    let (client, name) = database("recovery_shape").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    assert!(COLLECTIONS.contains(&RECOVERY_WORK));
    let rows = db.collection::<Document>(RECOVERY_WORK);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 0);
    for phase in ["admitted", "attaching", "completed", "superseded"] {
        let valid = carrier(phase);
        let key = valid.get_str("_id").unwrap();
        rows.replace_one(doc! {"_id":key}, valid.clone())
            .upsert(true)
            .await
            .unwrap();
        assert_eq!(
            rows.find_one(doc! {"_id":key}).await.unwrap().unwrap(),
            valid
        );
    }
    let baseline = carrier("admitted");
    let key = baseline.get_str("_id").unwrap();
    rows.replace_one(doc! {"_id":key}, baseline.clone())
        .await
        .unwrap();
    for case in 0..20 {
        let mut bad = baseline.clone();
        match case {
            0 => {
                bad.insert("format_version", 1_i64);
            }
            1 => {
                bad.insert("format_version", 2_i32);
            }
            2 => {
                bad.remove("result");
            }
            3 => {
                bad.insert("unexpected", true);
            }
            4 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .insert("unexpected", true);
            }
            5 => {
                bad.get_document_mut("result")
                    .unwrap()
                    .insert("unexpected", true);
            }
            6 => {
                bad.get_document_mut("result")
                    .unwrap()
                    .insert("agent_digest", "invalid");
            }
            7 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .get_document_mut("request")
                    .unwrap()
                    .insert("failure_seq", 2_f64);
            }
            8 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .insert("phase", "attaching");
            }
            9 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .insert("completed_at", "2026-11-11T00:00:00Z");
            }
            10 => {
                bad.get_document_mut("result")
                    .unwrap()
                    .insert("through_seq", 1_i64);
            }
            11 => {
                bad.insert("agent_id", "different-fixture");
            }
            12 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .insert("ready_at", "2026-11-11T00:00:00+00:00");
            }
            13 => {
                let target = bad
                    .get_document_mut("work")
                    .unwrap()
                    .get_document_mut("target")
                    .unwrap();
                target.insert("home_identity", "x".repeat(16_384));
                target
                    .get_document_mut("runtime")
                    .unwrap()
                    .insert("runtime_id", "y".repeat(16_384));
                assert!(bson::serialize_to_vec(&bad).unwrap().len() > 32 * 1024);
            }
            14 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .insert("format_version", 1_i64);
            }
            15 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .get_document_mut("request")
                    .unwrap()
                    .insert("failure_seq", 2_i32);
            }
            16 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .get_document_mut("target")
                    .unwrap()
                    .get_document_mut("runtime")
                    .unwrap()
                    .insert("boot_id", bson::Bson::Null);
            }
            17 => {
                bad.get_document_mut("work")
                    .unwrap()
                    .get_document_mut("target")
                    .unwrap()
                    .insert("active_turn", 3_i64);
            }
            18 => {
                bad = carrier("attaching");
                bad.get_document_mut("work")
                    .unwrap()
                    .get_document_mut("lease")
                    .unwrap()
                    .insert("fence", 2_i64);
            }
            _ => {
                bad = carrier("completed");
                bad.get_document_mut("result")
                    .unwrap()
                    .insert("published_at", "2026-11-10T00:00:00Z");
            }
        }
        invalid(rows.replace_one(doc! {"_id":key}, bad).await.unwrap_err());
        assert_eq!(
            rows.find_one(doc! {"_id":key}).await.unwrap().unwrap(),
            baseline
        );
    }
    schema::verify(&db).await.unwrap();
    db.drop().await.unwrap();
}
