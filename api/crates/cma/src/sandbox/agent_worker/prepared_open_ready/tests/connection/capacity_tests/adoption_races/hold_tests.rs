use super::*;
use mongodb::bson::Bson;
use mongodb::options::WriteConcern;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn sandbox_hold_fences_pending_adoption_admission_and_staging() {
    for (boundary, hold, admitted) in [
        (Boundary::BeforeAdmission, Bson::Null, false),
        (
            Boundary::Admitted,
            Bson::Document(doc! {"test_hold": true}),
            true,
        ),
    ] {
        let (fixture, full) = selected(None).await;
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let other = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let database = other.database(&fixture.name);
        let sandboxes = database.collection::<Document>("sandboxes");
        let agents = database.collection::<Document>("agents");
        let worktrees = database.collection::<Document>("worktrees");
        let admissions = database.collection::<Document>("agent_log_admissions");
        let evidence = database.collection::<Document>("agent_log_evidence");
        let sandbox_key = doc! {"_id": full.sandbox_id.as_str()};
        let agent_key = doc! {"_id": full.id.as_str()};
        let worktree_key = doc! {"_id": full.worktree_id.as_str()};
        let barrier = adoption_probe::arm(&fixture.name, boundary);
        let finish = tokio::spawn(async move {
            let result = finish_now(&fixture).await;
            (fixture, result)
        });
        wait(&barrier).await;
        assert_eq!(
            sandboxes
                .update_one(
                    sandbox_key.clone(),
                    doc! {"$set": {"ledger_maintenance": hold}}
                )
                .write_concern(WriteConcern::majority())
                .await
                .unwrap()
                .modified_count,
            1
        );
        let expected_sandbox = sandboxes.find_one(sandbox_key.clone()).await.unwrap();
        let expected_agent = agents.find_one(agent_key.clone()).await.unwrap().unwrap();
        let expected_worktree = worktrees.find_one(worktree_key.clone()).await.unwrap();
        assert_eq!(expected_agent.contains_key(WITNESS), admitted);
        assert_eq!(
            admissions.count_documents(doc! {}).await.unwrap(),
            u64::from(admitted)
        );
        assert_eq!(evidence.count_documents(doc! {}).await.unwrap(), 0);

        barrier.resume.notify_one();
        let (fixture, result) = finish.await.unwrap();
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        no_publication(&fixture).await;
        assert_eq!(
            sandboxes.find_one(sandbox_key).await.unwrap(),
            expected_sandbox
        );
        assert_eq!(
            agents.find_one(agent_key).await.unwrap(),
            Some(expected_agent)
        );
        assert_eq!(
            worktrees.find_one(worktree_key).await.unwrap(),
            expected_worktree
        );
        assert_eq!(
            admissions.count_documents(doc! {}).await.unwrap(),
            u64::from(admitted)
        );
        assert_eq!(evidence.count_documents(doc! {}).await.unwrap(), 0);
        fixture.cleanup().await;
    }
}
