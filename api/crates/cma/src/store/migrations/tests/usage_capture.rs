use super::*;

#[test]
fn usage_migration_has_explicit_additive_authority_and_no_activation() {
    let step = Step::SandboxUsageCapture;
    assert_eq!(Step::parse(sandbox_usage_capture::ID).unwrap(), step);
    assert_eq!(
        step.collections(),
        crate::store::sandbox_usage_export::COLLECTIONS
    );
    assert_eq!(step.checksum().len(), 64);
    for apply in [false, true] {
        let role = authority::role_for("fixture", apply, step);
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
        "authenticatedUserPrivileges":role.get_array("privileges").unwrap().clone()}};
        authority::verify_for(
            &status,
            "fixture",
            MongoAuthorityMode::Isolated,
            apply,
            step,
        )
        .unwrap();
        for grant in role.get_array("privileges").unwrap() {
            let actions = grant.as_document().unwrap().get_array("actions").unwrap();
            for forbidden in ["remove", "dropCollection", "dropIndex", "collMod"] {
                assert!(!actions
                    .iter()
                    .any(|action| action.as_str() == Some(forbidden)));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn usage_schema_plan_is_read_only_and_interrupted_migration_resumes() {
    let (client, database, legacy) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::SandboxUsageCapture);
    let plan = sandbox_usage_capture::inspect(&database).await.unwrap();
    assert_eq!(plan["writes"], false);
    assert_eq!(plan["backfill"], false);
    assert_eq!(plan["activation"], false);
    assert!(database.list_collection_names().await.unwrap().is_empty());
    ledger.initialize().await.unwrap();
    legacy.initialize().await.unwrap();
    ledger.claim(&identity()).await.unwrap();
    assert!(legacy.claim(&identity()).await.is_err());
    assert!(
        crate::store::sandbox_usage_export::schema::apply_next(&database)
            .await
            .unwrap()
    );
    expire(&database).await;
    assert_eq!(
        sandbox_usage_capture::apply(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "completed"
    );
    assert_eq!(
        sandbox_usage_capture::apply(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    for name in crate::store::sandbox_usage_export::COLLECTIONS {
        assert_eq!(
            database
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    database.drop().await.unwrap();
}
