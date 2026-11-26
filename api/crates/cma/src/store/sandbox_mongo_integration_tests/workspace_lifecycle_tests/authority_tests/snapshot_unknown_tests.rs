use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn renewed_snapshot_reconciles_lost_commit_acknowledgement_after_expiry() {
    let (admin, name) = database("work_renewed_unknown_commit").await;
    let app = format!("work-renewed-commit-{}", uuid::Uuid::new_v4());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&store).await;
    let mut original = store
        .claim_sandbox_work(
            &SandboxId(Kind::Sandbox.id().into()),
            "original",
            at(0),
            at(2),
        )
        .await
        .unwrap()
        .unwrap();
    let until = at(3);
    independent
        .renew_sandbox_work(&original, &at(0), &until)
        .await
        .unwrap()
        .unwrap();
    let revision = original.revision;
    original.revision += 1;
    original.name = "unknown-commit-result".into();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":5_i32},
            "data":{"failCommands":["commitTransaction"],"appName":&app,
                "blockConnection":true,"blockTimeMS":1500_i32,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                    "errmsg":"injected lost work publication acknowledgement"}}
        })
        .await
        .unwrap();
    let result = store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: revision,
            sandbox: Box::new(original.clone()),
            event_at: at(0),
            event: SandboxEventKind::WorkStepCompleted {
                step: original.work.step,
            },
        })
        .await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"off"
        })
        .await
        .unwrap();
    result.unwrap();
    let actual = independent
        .get_sandbox(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.work.lease.as_ref().unwrap().expires_at, until);
    assert!(
        until <= at(0),
        "reconciliation must finish after the original renewed expiry"
    );
    assert!(crate::store::work_lease_update::sandbox_matches(
        Some(&actual),
        &original
    ));
    assert_eq!(
        independent
            .sandbox_events(&original.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(independent
        .renew_sandbox_work(&original, &at(0), &at(15))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        independent.get_sandbox(&original.id).await.unwrap(),
        Some(actual)
    );
    admin.database(&name).drop().await.unwrap();
}
