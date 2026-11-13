use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn startup_refuses_missing_downgraded_or_malformed_retained_floor() {
    let (client, name) = database("work_floor_corrupt").await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_renewable_work(true);
    seed(&store).await;
    Kind::Sandbox
        .claim(&store, "protected", at(0), at(15))
        .await
        .unwrap()
        .unwrap();
    let rows = client
        .database(&name)
        .collection::<Document>("sandbox_credentials");
    let id = doc! {"_id":"ordinary-work-writer-floor-v1"};
    let original = rows.find_one(id.clone()).await.unwrap().unwrap();
    for change in [
        doc! {"protocol":"unknown"},
        doc! {"document_schema_version":1_i64},
    ] {
        rows.update_one(id.clone(), doc! {"$set":change})
            .await
            .unwrap();
        assert!(MongoSandboxStore::open(&client, &name).await.is_err());
        rows.replace_one(id.clone(), original.clone())
            .await
            .unwrap();
    }
    rows.delete_one(id).await.unwrap();
    assert!(MongoSandboxStore::open(&client, &name).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn short_claims_cap_server_expiry_and_retain_writer_floor_after_switch_off() {
    let (client, name) = database("work_floor_rollback").await;
    let enabled = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_renewable_work(true);
    let off = peer(&name).await;
    seed(&enabled).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        // An ahead clock cannot stretch the server's takeover bound.
        kind.claim(&enabled, "original", at(3600), at(3615))
            .await
            .unwrap()
            .unwrap();
        let protected = kind.row(&off).await;
        assert_eq!(protected.get_i64("document_schema_version").unwrap(), 3);
        assert_eq!(
            protected["record"]["work"]["authority_protocol"].as_str(),
            Some("renewed_v1")
        );
        let hello = client
            .database("admin")
            .run_command(doc! {"hello":1_i32})
            .await
            .unwrap();
        let server_ms = hello.get_datetime("localTime").unwrap().timestamp_millis();
        let expiry_ms =
            layer2_core::seconds_from_rfc3339(protected.get_str("work_lease_expires_at").unwrap())
                .unwrap() as i64
                * 1000;
        assert!((1..=15_000).contains(&(expiry_ms - server_ms)));
        // The old binary's real envelope decoder and version-one update shape
        // both reject this work; a feature switch cannot undo this floor.
        assert!(crate::store::sandbox_mongo::read_record::<serde_json::Value>(&protected).is_err());
        let legacy = kind
            .rows(&off)
            .update_one(
                doc! {"_id":kind.id(),"document_schema_version":1_i64},
                doc! {"$set":{"legacy_write_probe":true}},
            )
            .await
            .unwrap();
        assert_eq!(legacy.matched_count, 0);
        assert_eq!(kind.row(&off).await, protected);
        let expired = at(-1);
        kind.set(
            &off,
            doc! {"work_lease_expires_at":&expired,"record.work.lease.expires_at":&expired,
            "work_ready_at":at(-1),"record.work.ready_at":at(-1)},
        )
        .await;
        kind.claim(&off, "compatible-off-successor", at(0), at(1860))
            .await
            .unwrap()
            .unwrap();
        let successor = kind.row(&off).await;
        assert_eq!(successor.get_i64("document_schema_version").unwrap(), 3);
        assert!(successor.get_str("work_lease_expires_at").unwrap() <= at(15).as_str());
        assert_eq!(
            successor["record"]["work"]["lease"]["fence"].as_i64(),
            Some(2)
        );
        // Even the compatible expiry-only API must honor the retained floor.
        let until = match kind {
            Kind::Sandbox => {
                let row = off
                    .get_sandbox(&SandboxId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap();
                off.renew_sandbox_work(&row, &at(0), &at(1860)).await
            }
            Kind::Workspace => {
                let row = off
                    .get_workspace(&WorkspaceId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap();
                off.renew_workspace_work(&row, &at(0), &at(1860)).await
            }
        }
        .unwrap()
        .unwrap();
        assert!(until <= at(15));
    }
    // Reopening accepts retained protected work even with activation OFF.
    MongoSandboxStore::open(&client, &name).await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn never_activated_off_claims_keep_their_long_lease_and_version_one_shape() {
    let (client, name) = database("work_floor_never_on").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    seed(&store).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        let requested = at(1860);
        kind.claim(&store, "legacy", at(0), requested.clone())
            .await
            .unwrap()
            .unwrap();
        let row = kind.row(&store).await;
        assert_eq!(row.get_i64("document_schema_version").unwrap(), 1);
        assert_eq!(row.get_str("work_lease_expires_at").unwrap(), requested);
        assert!(row["record"]["work"]
            .as_document()
            .unwrap()
            .get("authority_protocol")
            .is_none());
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn aggregate_replacement_cannot_remove_a_retained_work_floor() {
    let (client, name) = database("work_floor_replacement").await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_renewable_work(true);
    seed(&store).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&store, "original", at(0), at(15))
            .await
            .unwrap()
            .unwrap();
        match kind {
            Kind::Sandbox => {
                let mut row = store
                    .get_sandbox(&SandboxId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap();
                let revision = row.revision;
                row.revision += 1;
                row.name = "metadata-change".into();
                row.work.authority_protocol = None;
                store.save_sandbox(revision, row).await.unwrap();
                assert!(store
                    .get_sandbox(&SandboxId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap()
                    .work
                    .authority_protocol
                    .is_some());
            }
            Kind::Workspace => {
                let mut row = store
                    .get_workspace(&WorkspaceId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap();
                let revision = row.revision;
                row.revision += 1;
                row.name = "metadata-change".into();
                row.work.authority_protocol = None;
                store.save_workspace(revision, row).await.unwrap();
                assert!(store
                    .get_workspace(&WorkspaceId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap()
                    .work
                    .authority_protocol
                    .is_some());
            }
        }
        assert_eq!(
            kind.row(&store)
                .await
                .get_i64("document_schema_version")
                .unwrap(),
            3
        );
    }
    client.database(&name).drop().await.unwrap();
}
