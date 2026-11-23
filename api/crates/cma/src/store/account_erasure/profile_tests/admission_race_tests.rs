use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_sandbox_admission_cannot_commit_a_profile_erased_after_resolution() {
    use futures_util::TryStreamExt;
    use layer2_core::sandbox::{
        CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy,
        SandboxService,
    };
    use layer2_core::{Authorization, Caller, Capability};
    use std::sync::Arc;
    let seed = seeded("erasure_admission_fence").await;
    let source = seed.client.database(&seed.database);
    let (observed, independent, notice, release) =
        super::race_tests::intercepted("update", "sandbox_admissions").await;
    for name in [
        "agent_profiles",
        "workspace_profiles",
        "sandbox_profiles",
        "agent_profile_revisions",
        "workspace_profile_revisions",
        "sandbox_profile_revisions",
    ] {
        let mut cursor = source
            .collection::<Document>(name)
            .find(doc! {})
            .await
            .unwrap();
        while let Some(row) = cursor.try_next().await.unwrap() {
            independent
                .collection::<Document>(name)
                .insert_one(row)
                .await
                .unwrap();
        }
    }
    let profiles = Arc::new(
        crate::store::sandbox_mongo::MongoProfileStore::open(
            independent.client(),
            independent.name(),
        )
        .await
        .unwrap(),
    );
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(observed.client(), observed.name())
            .await
            .unwrap(),
    );
    let service = SandboxService::new(
        store,
        profiles,
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("visitor"),
        Authorization::new([Capability::CreateSandbox]),
    );
    let revision = seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let request = CreateSandbox {
        profile: seed.target.sandbox_profile.clone(),
        name: Some("late-catalogue-admission".into()),
        confirmed_digest: Some(revision.digest),
        origin: SandboxOrigin::Manual,
    };
    let task = tokio::spawn(async move { service.create(&caller, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    erase(&independent, &seed.owner).await;
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "a resolved catalogue pin must be fenced in the admission transaction"
    );
    assert_eq!(
        independent
            .collection::<Document>("sandboxes")
            .count_documents(doc! {"owner":"visitor"})
            .await
            .unwrap(),
        0
    );
    independent.drop().await.unwrap();
    source.drop().await.unwrap();
}
