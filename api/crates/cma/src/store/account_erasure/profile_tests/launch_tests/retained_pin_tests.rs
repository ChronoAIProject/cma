use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn anonymized_profile_pins_admit_foreign_launches_and_missing_revisions_are_unavailable() {
    let seed = seeded("erasure_foreign_publisher_launch").await;
    let db = seed.client.database(&seed.database);
    let mut workspace = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    workspace.id = layer2_core::WorkspaceProfileId("wkp_retained_standalone".into());
    workspace.draft.setup_commands = vec!["prepare-workspace".into()];
    seed.profiles
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    let workspace_revision = seed
        .profiles
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();
    let mut target = seed.target.clone();
    target.workspace_profile = layer2_core::profiles::WorkspaceProfileRef::new(workspace.id, 1);
    let other_publisher = SubjectId::new("surviving-publisher");
    let id = TriggerId("trg_foreign_profile_launch".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &other_publisher, &target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &other_publisher, 1, NOW.into())
        .await
        .unwrap();
    let resources = Arc::new(
        crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap(),
    );
    let definitions = Arc::new(seed.store.clone());
    let service = LaunchService::new(
        definitions.clone(),
        seed.profiles.clone(),
        definitions,
        resources,
        Arc::new(Connected),
        Arc::new(LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
            visitor_launches_per_day: 5,
        }),
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("independent-visitor"),
        Authorization::new([Capability::CreateSandbox, Capability::LaunchTriggers]),
    );
    let before_erasure = service
        .create_or_resume(
            &caller,
            &public.public_id,
            request("before-profile-erasure"),
        )
        .await
        .unwrap();
    erase(&db, &seed.owner).await;
    let created = service
        .create_or_resume(&caller, &public.public_id, request("anonymous-pins"))
        .await
        .unwrap();
    assert_eq!(created.launch.state, LaunchState::Draft);
    let disclosures = &created.launch.trust.disclosures;
    assert_eq!(disclosures.len(), 2);
    assert_eq!(disclosures[0].name, "Sandbox");
    assert_eq!(disclosures[0].commands, ["apt-get update"]);
    assert_eq!(
        Some(&disclosures[0].commands_digest),
        created.launch.trust.sandbox_digest.as_ref()
    );
    assert_eq!(disclosures[1].name, "Workspace");
    assert_eq!(disclosures[1].commands, ["prepare-workspace"]);
    assert_eq!(disclosures[1].commands_digest, workspace_revision.digest);
    assert!(created.launch.review.trust_digest().is_some());
    assert!(matches!(
        service
            .start(
                &caller,
                &created.launch.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: vec![],
                },
            )
            .await,
        Err(LaunchServiceError::TrustConfirmationMismatch)
    ));
    let accepted = service
        .start(
            &caller,
            &created.launch.id,
            StartLaunch {
                trust_confirmed_digest: created.launch.review.trust_digest().map(str::to_owned),
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(accepted.state, LaunchState::Resolving);
    assert_eq!(accepted.work.step, LaunchWorkStep::ResolveSandbox);
    assert_eq!(accepted.owner, caller.subject);
    let resumed = service
        .start(
            &caller,
            &before_erasure.launch.id,
            StartLaunch {
                trust_confirmed_digest: before_erasure
                    .launch
                    .review
                    .trust_digest()
                    .map(str::to_owned),
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(resumed.state, LaunchState::Resolving);
    assert_eq!(resumed.trust, before_erasure.launch.trust);
    db.collection::<Document>("agent_profile_revisions")
        .delete_one(doc! {
            "profile_id":seed.target.agent_profile.id.as_str(),
            "revision":i64::from(seed.target.agent_profile.revision),
        })
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_or_resume(&caller, &public.public_id, request("missing-anonymous-pin"))
            .await,
        Err(LaunchServiceError::Unavailable(_))
    ));
    assert_eq!(service.read(&caller, &accepted.id).await.unwrap(), accepted);
    db.drop().await.unwrap();
}
