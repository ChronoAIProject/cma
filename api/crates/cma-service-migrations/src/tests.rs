use super::*;
use mongodb::{Client, IndexModel};

const FIRST: Manifest = Manifest {
    id: "fixture-v1",
    prerequisites: &[],
    schema_fingerprint: [0; 32],
    definition: "fixture validator; owner index; version 1",
};
const SECOND: Manifest = Manifest {
    id: "fixture-v2",
    prerequisites: &[FIRST.id],
    schema_fingerprint: [0; 32],
    definition: "second step",
};
const MANIFESTS: &[Manifest] = &[FIRST, SECOND];
pub(super) const LEDGER: &str = "migration_fixture_ledger";
const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const IMAGE: &str = "example.invalid/fixture@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn release() -> Release<'static> {
    Release {
        source: SOURCE,
        image: IMAGE,
    }
}

#[test]
fn manifest_checksum_covers_order_and_definition_and_release_requires_digest() {
    assert_ne!(
        FIRST.checksum(),
        Manifest {
            definition: "changed",
            ..FIRST
        }
        .checksum()
    );
    assert_ne!(
        SECOND.checksum(),
        Manifest {
            prerequisites: &[],
            ..SECOND
        }
        .checksum()
    );
    assert!(release().validate("fixture").is_ok());
    assert!(release().validate("other").is_err());
    assert!(Release {
        source: SOURCE,
        image: "example.invalid/fixture:latest"
    }
    .validate("fixture")
    .is_err());
}

pub(super) async fn fixture() -> Database {
    let client = Client::with_uri_str(std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "migration_fixture_{}",
        uuid::Uuid::new_v4().simple()
    ));
    grant_fixture_operator(&db, LEDGER).await;
    db
}
async fn grant_fixture_operator(db: &Database, ledger: &str) {
    db.run_command(operator_role(db.name(), ledger))
        .await
        .unwrap();
    let message = db.run_command(doc! {"connectionStatus":1}).await.unwrap();
    let user = message
        .get_document("authInfo")
        .unwrap()
        .get_array("authenticatedUsers")
        .unwrap()[0]
        .as_document()
        .unwrap();
    db.client().database(user.get_str("db").unwrap()).run_command(doc! {"grantRolesToUser":user.get_str("user").unwrap(),"roles":[{"role":format!("{ledger}_operator"),"db":db.name()}]}).await.unwrap();
}
async fn expire(runner: &Runner) {
    runner
        .rows
        .update_one(
            doc! {"_id":"runner"},
            doc! {"$set":{"expires_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}
async fn converge(db: &Database) -> Result<()> {
    marker(db, "fixture_markers", "schema", "version", &[1], 1)
        .await
        .map_err(database_error)?;
    collection(
        db,
        "fixture_rows",
        doc! {"$jsonSchema":{"bsonType":"object","required":["owner"]}},
        true,
    )
    .await
    .map_err(database_error)?;
    indexes(
        db,
        "fixture_rows",
        vec![IndexModel::builder().keys(doc! {"owner":1}).build()],
    )
    .await
    .map_err(database_error)?;
    update(
        db,
        "fixture_markers",
        doc! {"_id":"schema"},
        doc! {"$set":{"version":1}},
        true,
        false,
    )
    .await
    .map_err(database_error)?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn dry_run_partial_ddl_rerun_and_drift() {
    let db = fixture().await;
    let runner = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    let definition = describe(converge(&db)).await.unwrap();
    assert_ne!(definition, [0; 32]);
    assert_eq!(definition, describe(converge(&db)).await.unwrap());
    assert_ne!(
        FIRST.checksum(),
        Manifest {
            schema_fingerprint: definition,
            ..FIRST
        }
        .checksum()
    );
    let ready = inspect(converge(&db)).await.unwrap();
    assert!(!ready);
    assert_eq!(runner.plan(FIRST, ready).await.unwrap()["writes"], false);
    assert_eq!(runner.status(FIRST).await.unwrap()["completed"], false);
    assert!(db.list_collection_names().await.unwrap().is_empty());

    let legacy = runner.claim(FIRST, &release(), true).await;
    assert!(legacy.is_err());
    runner.initialize().await.unwrap();
    let legacy = runner.claim(FIRST, &release(), true).await.unwrap();
    assert!(runner
        .activate("fixture drained legacy writers")
        .await
        .is_err());
    runner.finish(&legacy, true, true).await.unwrap();
    runner
        .activate("fixture drained legacy writers")
        .await
        .unwrap();
    assert!(runner
        .legacy(FIRST, || async { Ok(false) }, async {
            panic!("legacy migration crossed cutover")
        })
        .await
        .is_err());
    assert!(runner.claim(SECOND, &release(), false).await.is_err());
    assert!(runner
        .apply(
            FIRST,
            "invalid",
            release(),
            || inspect(converge(&db)),
            || converge(&db)
        )
        .await
        .is_err());

    let result = runner
        .apply(
            FIRST,
            &FIRST.checksum(),
            release(),
            || inspect(converge(&db)),
            || async {
                collection(
                    &db,
                    "fixture_rows",
                    doc! {"$jsonSchema":{"bsonType":"object","required":["owner"]}},
                    true,
                )
                .await
                .map_err(database_error)?;
                Err("fixture interruption after acknowledged DDL")
            },
        )
        .await;
    assert!(result.is_err());
    assert!(db
        .list_collection_names()
        .await
        .unwrap()
        .contains(&"fixture_rows".into()));
    assert!(runner.pending_effect().await.unwrap().is_none());
    assert_eq!(
        runner
            .apply(
                FIRST,
                &FIRST.checksum(),
                release(),
                || inspect(converge(&db)),
                || converge(&db)
            )
            .await
            .unwrap()["outcome"],
        "completed"
    );
    assert_eq!(
        runner
            .apply(
                FIRST,
                &FIRST.checksum(),
                release(),
                || inspect(converge(&db)),
                || async { panic!("completed migration reran") }
            )
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    db.run_command(doc! {"collMod":"fixture_rows","validator":doc!{}})
        .await
        .unwrap();
    assert!(runner
        .apply(
            FIRST,
            &FIRST.checksum(),
            release(),
            || inspect(converge(&db)),
            || async { panic!("drift was overwritten") }
        )
        .await
        .is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn concurrent_claim_fence_and_dispatched_command_reconciliation() {
    let db = fixture().await;
    let first = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    let second = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    first
        .activate("fixture drained legacy writers")
        .await
        .unwrap();
    let release = release();
    let (a, b) = tokio::join!(
        first.claim(FIRST, &release, false),
        second.claim(FIRST, &release, false)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let old = a.or(b).unwrap();
    expire(&first).await;
    let current = second.claim(FIRST, &release, false).await.unwrap();
    assert!(current.fence > old.fence);
    let command = doc! {"collMod":"fixture_rows","validator":{"owner":{"$exists":true}}};
    assert!(first
        .begin_effect(&old, "stale", "collMod", &command)
        .await
        .is_err());
    second
        .begin_effect(&current, "pending", "collMod", &command)
        .await
        .unwrap();
    // Pause immediately after dispatch authority; no amount of lease expiry can
    // prove that the old process or its server command has drained.
    expire(&second).await;
    assert!(first.claim(FIRST, &release, false).await.is_err());
    assert!(first
        .reconcile(
            FIRST,
            &current.attempt,
            current.fence + 1,
            "pending",
            "fixture evidence"
        )
        .await
        .is_err());
    assert!(first
        .reconcile(
            FIRST,
            &current.attempt,
            current.fence,
            "different",
            "fixture evidence"
        )
        .await
        .is_err());
    assert_eq!(
        first
            .pending_effect()
            .await
            .unwrap()
            .unwrap()
            .get_document("pending_command")
            .unwrap(),
        &command
    );
    first
        .reconcile(
            FIRST,
            &current.attempt,
            current.fence,
            "pending",
            "fixture evidence of drained submitter and server command",
        )
        .await
        .unwrap();
    let resumed = first.claim(FIRST, &release, false).await.unwrap();
    assert!(resumed.fence > current.fence);
    assert!(second
        .end_effect(&current, "pending", "acknowledged")
        .await
        .is_err());
    assert!(second
        .begin_effect(&current, "stale", "collMod", &command)
        .await
        .is_err());
    first.finish(&resumed, false, false).await.unwrap();
    assert!(first
        .reconcile(
            FIRST,
            &current.attempt,
            current.fence,
            "pending",
            "stale receipt"
        )
        .await
        .is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn unknown_history_refuses_older_migrator_and_service_ledgers_are_isolated() {
    let db = fixture().await;
    let runner = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    runner.initialize().await.unwrap();
    runner.rows.insert_one(doc!{"_id":"future","kind":"completion","schema":SCHEMA,"service":"fixture","migration":"future-v3","checksum":"new"}).await.unwrap();
    assert!(runner.status(FIRST).await.is_err());
    assert!(runner
        .legacy(FIRST, || async { Ok(false) }, async {
            panic!("old migrator crossed future history")
        })
        .await
        .is_err());
    let separate = Runner::new(&db, "other_fixture_ledger", "other", MANIFESTS);
    assert!(separate.status(FIRST).await.is_ok());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn startup_refuses_own_unresolved_final_marker_and_observes_only_fenced_completion() {
    let db = fixture().await;
    let runner = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    let check = || async {
        Ok(db
            .collection::<Document>("fixture_markers")
            .find_one(doc! {"_id":"schema","version":1})
            .await
            .map_err(database_error)?
            .is_some())
    };
    let result = runner
        .legacy_startup(FIRST, check, async {
            let rows = db.collection::<Document>("fixture_markers");
            effect(
                &db,
                doc! {"update":"fixture_markers","fixture":"final marker"},
                async {
                    rows.update_one(doc! {"_id":"schema"}, doc! {"$set":{"version":1}})
                        .upsert(true)
                        .await?;
                    Err::<(), _>(mongodb::error::Error::custom(
                        "fixture lost acknowledgement",
                    ))
                },
            )
            .await
            .map_err(database_error)
        })
        .await;
    assert!(result.is_err());
    assert!(check().await.unwrap());
    assert!(runner.pending_effect().await.unwrap().is_some());
    assert_eq!(
        runner.status(FIRST).await.unwrap()["latest_attempt"]["outcome"],
        "failed"
    );
    assert!(runner
        .legacy_startup(FIRST, check, async {
            panic!("unresolved dispatch cannot be replaced")
        })
        .await
        .is_err());
    db.drop().await.unwrap();

    let db = fixture().await;
    let runner = Runner::new(&db, LEDGER, "fixture", MANIFESTS);
    runner.initialize().await.unwrap();
    let lease = runner.claim(FIRST, &release(), true).await.unwrap();
    let command = doc! {"update":"fixture_markers","fixture":"final marker"};
    runner
        .begin_effect(&lease, "pending", "update", &command)
        .await
        .unwrap();
    db.collection::<Document>("fixture_markers")
        .update_one(doc! {"_id":"schema"}, doc! {"$set":{"version":1}})
        .upsert(true)
        .await
        .unwrap();
    let check = || async {
        Ok(db
            .collection::<Document>("fixture_markers")
            .find_one(doc! {"_id":"schema","version":1})
            .await
            .map_err(database_error)?
            .is_some())
    };
    let observer = runner.legacy_startup(FIRST, check, async {
        panic!("competing attempt must not dispatch")
    });
    tokio::pin!(observer);
    // A visible marker with its dispatch unresolved is insufficient.
    assert!(
        tokio::time::timeout(Duration::from_millis(400), &mut observer)
            .await
            .is_err()
    );
    runner
        .end_effect(&lease, "pending", "acknowledged")
        .await
        .unwrap();
    runner.finish(&lease, true, true).await.unwrap();
    // The other replica has already advanced to its next manifest before the
    // observer is resumed. The first exact completed receipt remains durable.
    let next = runner.claim(SECOND, &release(), true).await.unwrap();
    assert!(next.fence > lease.fence);
    observer.await.unwrap();
    assert_eq!(
        runner
            .rows
            .count_documents(doc! {"kind":"attempt","migration":FIRST.id})
            .await
            .unwrap(),
        1
    );
    let second_check = || async {
        Ok(db
            .collection::<Document>("fixture_markers")
            .find_one(doc! {"_id":"second","version":1})
            .await
            .map_err(database_error)?
            .is_some())
    };
    let second_observer = runner.legacy_startup(SECOND, second_check, async {
        panic!("the second manifest must not dispatch twice")
    });
    tokio::pin!(second_observer);
    assert!(
        tokio::time::timeout(Duration::from_millis(400), &mut second_observer)
            .await
            .is_err()
    );
    runner
        .begin_effect(&next, "next", "update", &command)
        .await
        .unwrap();
    db.collection::<Document>("fixture_markers")
        .insert_one(doc! {"_id":"second","version":1})
        .await
        .unwrap();
    runner
        .end_effect(&next, "next", "acknowledged")
        .await
        .unwrap();
    runner.finish(&next, true, true).await.unwrap();
    second_observer.await.unwrap();
    assert_eq!(
        runner
            .rows
            .count_documents(doc! {"kind":"attempt","migration":SECOND.id})
            .await
            .unwrap(),
        1
    );
    assert!(check().await.unwrap() && second_check().await.unwrap());

    db.drop().await.unwrap();
}
