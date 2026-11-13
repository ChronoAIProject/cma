//! Surviving publishers can still admit visitor Launches using anonymous pins.
use super::*;
use layer2_core::trigger_domain::*;
use layer2_core::{Authorization, Caller, Capability, ServiceId};
use std::sync::Arc;

struct Connected;
#[async_trait::async_trait]
impl ServiceConnectionBroker for Connected {
    async fn requirements(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect())
    }
    async fn create_handoff(
        &self,
        _: &SubjectId,
        _: &ServiceId,
        _: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
    async fn handoff_state(
        &self,
        _: &SubjectId,
        _: &str,
        _: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
}

fn request(key: &str) -> CreateOrResumeLaunch {
    CreateOrResumeLaunch {
        placement_id: None,
        referrer_origin: None,
        start_if_ready: false,
        choice: None,
        idempotency_key: key.into(),
    }
}

struct NoRuntime;
#[async_trait::async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for NoRuntime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        Err(layer2_core::sandbox::AgentRuntimeError::NotFound)
    }
    async fn fence(
        &self,
        _: &layer2_core::AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn accepted_foreign_trigger_launch_creates_resources_after_profile_owner_erasure() {
    use layer2_core::sandbox::{
        CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy,
        SandboxService,
    };
    let seed = seeded("erasure_foreign_launch_continuation").await;
    let db = seed.client.database(&seed.database);
    let publisher = SubjectId::new("surviving-publisher");
    let id = TriggerId("trg_foreign_frozen_continuation".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &publisher, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &publisher, 1, NOW.into())
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
        resources.clone(),
        Arc::new(Connected),
        Arc::new(LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
        }),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("independent-visitor"),
        Authorization::new([
            Capability::CreateSandbox,
            Capability::LaunchTriggers,
            Capability::UseOwnSandboxes,
        ]),
    );
    let created = service
        .create_or_resume(&caller, &public.public_id, request("frozen-continuation"))
        .await
        .unwrap();
    let launch = service
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
    assert_eq!(launch.state, LaunchState::Resolving);
    let sandboxes = SandboxService::new(
        resources.clone(),
        seed.profiles.clone(),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let activity = crate::store::MongoActivityStore::open(
        &seed.client,
        &seed.database,
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let settings = Arc::new(layer2_core::PlatformSettingsService::new(
        Arc::new(
            crate::store::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
                .await
                .unwrap(),
        ),
        layer2_core::ActivityService::new(Arc::new(activity)),
    ));
    let driver = crate::trigger::ServiceLaunchResourceDriver::new(
        crate::trigger::ServiceLaunchResourceDependencies {
            definitions: Arc::new(seed.store.clone()),
            profiles: seed.profiles.clone(),
            resources: resources.clone(),
            settings,
            connections: Arc::new(Connected),
            sandboxes: sandboxes.clone(),
            workspaces: layer2_core::sandbox::WorkspaceService::new(
                resources.clone(),
                seed.profiles.clone(),
            ),
            agents: layer2_core::sandbox::AgentService::new(
                resources.clone(),
                seed.profiles.clone(),
                Arc::new(NoRuntime),
                layer2_core::RuntimeCapabilities::new([
                    layer2_core::RuntimeCapability::Conversation,
                ]),
            ),
        },
    );
    let worker = crate::trigger::LaunchWorker::new(
        Arc::new(seed.store.clone()),
        driver,
        crate::trigger::LaunchWorkerConfig::new("foreign-continuation"),
    );
    let now = std::time::SystemTime::now();
    for name in ["sandbox_profiles", "workspace_profiles", "agent_profiles"] {
        crate::store::durability::majority_collection::<Document>(&db, name)
            .update_many(
                doc! {"owner":seed.owner.as_str()},
                doc! {"$set":{"account_erasure_fenced":true},"$currentDate":{"account_erasure_fenced_at":true}},
            )
            .await
            .unwrap();
    }
    assert!(worker.run_one(&launch.id, now).await.unwrap());
    let pending = service.read(&caller, &launch.id).await.unwrap();
    assert_eq!(pending.state, LaunchState::Resolving);
    assert!(pending.failure.is_none());
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(
        pending.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired,
    );
    for name in ["sandboxes", "workspaces", "agents"] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    erase(&db, &seed.owner).await;
    let launch_rows = db.collection::<Document>("trigger_launches");
    let saved = launch_rows
        .find_one(doc! {"_id":launch.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    seed.store
        .claim_launch(
            &launch.id,
            "negative-proof-control",
            layer2_core::rfc3339_at(now + std::time::Duration::from_secs(10)),
            layer2_core::rfc3339_at(now + std::time::Duration::from_secs(600)),
        )
        .await
        .unwrap()
        .unwrap();
    let create_frozen = |launch_id: &str| CreateSandbox {
        profile: seed.target.sandbox_profile.clone(),
        name: Some("negative-frozen-control".into()),
        confirmed_digest: launch.trust.sandbox_digest.clone(),
        origin: SandboxOrigin::Trigger {
            launch_id: launch_id.into(),
        },
    };
    assert!(sandboxes
        .create_for_launch(
            &caller,
            Capability::CreateSandbox,
            create_frozen("lnch_missing_proof")
        )
        .await
        .is_err());
    let other = Caller::new(
        SubjectId::new("different-visitor"),
        Authorization::new([Capability::CreateSandbox]),
    );
    assert!(sandboxes
        .create_for_launch(
            &other,
            Capability::CreateSandbox,
            create_frozen(launch.id.as_str())
        )
        .await
        .is_err());
    let definitions = db.collection::<Document>("trigger_definitions");
    definitions
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    assert!(sandboxes
        .create_for_launch(
            &caller,
            Capability::CreateSandbox,
            create_frozen(launch.id.as_str())
        )
        .await
        .is_err());
    definitions
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$unset":{"account_erasure_fenced":""}},
        )
        .await
        .unwrap();
    launch_rows
        .update_one(
            doc! {"_id":launch.id.as_str()},
            doc! {"$set":{"public_id":"pub_mismatched_proof"}},
        )
        .await
        .unwrap();
    assert!(sandboxes
        .create_for_launch(
            &caller,
            Capability::CreateSandbox,
            create_frozen(launch.id.as_str())
        )
        .await
        .is_err());
    launch_rows
        .replace_one(doc! {"_id":launch.id.as_str()}, saved)
        .await
        .unwrap();
    assert_eq!(
        db.collection::<Document>("sandboxes")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut continued = launch.clone();
    for step in 1..13 {
        worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(step * 10))
            .await
            .unwrap();
        continued = service.read(&caller, &launch.id).await.unwrap();
        assert!(
            continued.failure.is_none(),
            "frozen continuation failed: {:?}",
            continued.failure
        );
        if let Some(id) = continued.resolution.sandbox.id() {
            let mut sandbox = resources.get_sandbox(id).await.unwrap().unwrap();
            assert!(!sandbox.name.is_empty());
            if sandbox.status == layer2_core::sandbox::SandboxStatus::Provisioning {
                let ready =
                    crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
                        id.as_str(),
                    );
                sandbox.status = ready.status;
                sandbox.runtime = ready.runtime;
                sandbox.lifecycle = ready.lifecycle;
                let mut credential = ready.credential;
                credential.source = sandbox.credential.source.clone();
                credential.reaches = sandbox.credential.reaches.clone();
                credential.sign_in = sandbox.credential.sign_in;
                sandbox.credential = credential;
                sandbox.work = ready.work;
                sandbox.revision += 1;
                sandbox.validate().unwrap();
                resources
                    .sandboxes
                    .update_one(
                        doc! {"_id":id.as_str()},
                        doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
                    )
                    .await
                    .unwrap();
                assert_eq!(resources.get_sandbox(id).await.unwrap(), Some(sandbox));
            }
        }
        if let Some(id) = continued.resolution.workspace.id() {
            use layer2_core::sandbox::store::WorkspaceStore;
            let mut workspace = resources.get_workspace(id).await.unwrap().unwrap();
            if workspace.status == layer2_core::sandbox::WorkspaceStatus::Preparing {
                workspace.status = layer2_core::sandbox::WorkspaceStatus::Ready;
                workspace.ready_at = Some(layer2_core::now_rfc3339());
                workspace.resolved_commit = Some("a".repeat(40));
                workspace.work = layer2_core::sandbox::WorkspaceWorkState::new(
                    layer2_core::sandbox::WorkspaceWorkStep::None,
                    layer2_core::now_rfc3339(),
                );
                workspace.revision += 1;
                resources
                    .workspaces
                    .replace_one(
                        doc! {"_id":id.as_str()},
                        crate::store::workspaces::workspace_document(&workspace).unwrap(),
                    )
                    .await
                    .unwrap();
            }
        }
        if continued.resolution.agent.id().is_some() {
            break;
        }
    }
    assert!(
        continued.resolution.agent.id().is_some(),
        "worker did not create the frozen Agent: {continued:?}"
    );
    assert!(sandboxes
        .create(
            &caller,
            CreateSandbox {
                profile: seed.target.sandbox_profile.clone(),
                name: Some("new-direct-pin".into()),
                confirmed_digest: launch.trust.sandbox_digest.clone(),
                origin: SandboxOrigin::Manual,
            }
        )
        .await
        .is_err());
    assert!(seed.store.trigger(&id, &publisher).await.unwrap().is_some());
    use layer2_core::profiles::{MarketplaceLocator, MarketplaceStore};
    assert!(seed
        .profiles
        .fork_profile(
            &caller.subject,
            &MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
            seed.target.sandbox_profile.revision,
            layer2_core::now_rfc3339(),
        )
        .await
        .is_err());
    db.drop().await.unwrap();
}

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
