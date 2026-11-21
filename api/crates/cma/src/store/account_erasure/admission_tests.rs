//! Real Mongo coverage for catalogue pin fences and snapshot compatibility.
mod worker_tests;

use layer2_core::profiles::{
    AgentProfileStore, ProfileCapacityBounds, ProfileStoreError, SandboxProfileStore,
    WorkspaceProfileStore,
};
use layer2_core::sandbox::{
    CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy, SandboxService,
};
use layer2_core::trigger_domain::{TriggerId, TriggerStore};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use mongodb::bson::{doc, Document};
use std::sync::Arc;

use crate::store::trigger_definitions::integration_tests::{seeded, trigger};

const NOW: &str = "2026-11-14T00:00:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_sandbox_pins_require_unfenced_root_and_inherited_profiles() {
    let seed = seeded("erasure_catalogue_pin_fences").await;
    let db = seed.client.database(&seed.database);
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap(),
    );
    let service = SandboxService::new(
        store,
        seed.profiles.clone(),
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
    let request = |name: &str| CreateSandbox {
        profile: seed.target.sandbox_profile.clone(),
        name: Some(name.into()),
        confirmed_digest: Some(revision.digest.clone()),
        origin: SandboxOrigin::Manual,
    };
    service
        .create(&caller, request("admitted-before-fence"))
        .await
        .unwrap();
    for (collection, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        assert!(
            service.create(&caller, request(collection)).await.is_err(),
            "new catalogue pin accepted fenced {collection}"
        );
        assert_eq!(
            db.collection::<Document>("sandboxes")
                .count_documents(doc! {"owner":"visitor"})
                .await
                .unwrap(),
            1
        );
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fenced_profile_cannot_save_or_publish_and_unfenced_profile_can() {
    let seed = seeded("erasure_profile_publication_fence").await;
    let db = seed.client.database(&seed.database);
    let id = &seed.target.agent_profile.id;
    let mut profile = seed.profiles.agent_profile(id).await.unwrap().unwrap();
    profile.draft_version = 2;
    profile.updated_at = NOW.into();
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.profiles.save_agent_draft(1, profile.clone()).await,
        Err(ProfileStoreError::Conflict)
    ));
    assert!(matches!(
        seed.profiles.publish_agent_profile(id, 1, NOW.into()).await,
        Err(ProfileStoreError::Conflict)
    ));
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$unset":{"account_erasure_fenced":""}},
        )
        .await
        .unwrap();
    seed.profiles.save_agent_draft(1, profile).await.unwrap();
    seed.profiles
        .publish_agent_profile(id, 2, NOW.into())
        .await
        .unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_and_trigger_publication_cannot_add_fenced_child_pins() {
    let seed = seeded("erasure_child_publication_fences").await;
    let db = seed.client.database(&seed.database);
    let agents = db.collection::<Document>("agent_profiles");
    agents
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.profiles
            .publish_workspace_profile(&seed.target.workspace_profile.id, 1, NOW.into())
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    agents
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$unset":{"account_erasure_fenced":""}},
        )
        .await
        .unwrap();
    seed.profiles
        .publish_workspace_profile(&seed.target.workspace_profile.id, 1, NOW.into())
        .await
        .unwrap();
    let workspaces = db.collection::<Document>("workspace_profiles");
    workspaces
        .update_one(
            doc! {"_id":seed.target.workspace_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    let bounds = ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    };
    assert!(matches!(
        seed.profiles
            .publish_sandbox_profile(&seed.target.sandbox_profile.id, 1, NOW.into(), bounds, &[])
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    workspaces
        .update_one(
            doc! {"_id":seed.target.workspace_profile.id.as_str()},
            doc! {"$unset":{"account_erasure_fenced":""}},
        )
        .await
        .unwrap();
    seed.profiles
        .publish_sandbox_profile(&seed.target.sandbox_profile.id, 1, NOW.into(), bounds, &[])
        .await
        .unwrap();
    let id = TriggerId("trg_fenced_publication".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    for (collection, profile_id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":profile_id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        assert!(seed
            .store
            .publish_trigger(&id, &seed.owner, 1, NOW.into())
            .await
            .is_err());
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":profile_id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    seed.store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_workspace_and_agent_admissions_reject_fenced_pins_but_inline_workspace_survives()
{
    use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};
    use layer2_core::sandbox::{
        AgentOrigin, AgentService, CreateAgent, CreateInlineWorkspace, CreateWorkspace,
        InlineWorkspaceSource, WorkspaceService, WorkspaceStatus, WorkspaceWorkState,
        WorkspaceWorkStep,
    };
    let seed = seeded("erasure_workspace_agent_pins").await;
    let db = seed.client.database(&seed.database);
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap(),
    );
    let mut parent = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_pin_parent",
    );
    parent.owner = SubjectId::new("visitor");
    store.create_sandbox(parent.clone()).await.unwrap();
    let caller = Caller::new(
        parent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let workspaces = WorkspaceService::new(store.clone(), seed.profiles.clone());
    let request = CreateWorkspace {
        profile: seed.target.workspace_profile.clone(),
        confirmed_digest: None,
    };
    for (collection, id) in [
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        assert!(workspaces
            .create(&caller, &parent.id, request.clone())
            .await
            .is_err());
        assert_eq!(
            db.collection::<Document>("workspaces")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    let mut workspace = workspaces
        .create(&caller, &parent.id, request)
        .await
        .unwrap();
    let expected = workspace.revision;
    workspace.revision += 1;
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    store
        .save_workspace(expected, workspace.clone())
        .await
        .unwrap();
    let agents = AgentService::new(
        store.clone(),
        seed.profiles.clone(),
        Arc::new(NoRuntime),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    );
    let request = CreateAgent {
        profile: seed.target.agent_profile.clone(),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
    };
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    assert!(agents
        .create(&caller, &workspace.id, request.clone())
        .await
        .is_err());
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$unset":{"account_erasure_fenced":""}},
        )
        .await
        .unwrap();
    agents
        .create(&caller, &workspace.id, request)
        .await
        .unwrap();
    for collection in [
        "workspace_profiles",
        "agent_profiles",
        "workspace_profile_revisions",
        "agent_profile_revisions",
    ] {
        db.collection::<Document>(collection)
            .delete_many(doc! {})
            .await
            .unwrap();
    }
    workspaces
        .create_inline(
            &caller,
            &parent.id,
            CreateInlineWorkspace {
                name: Some("inline-after-erasure".into()),
                source: InlineWorkspaceSource::Empty,
            },
        )
        .await
        .unwrap();
    db.drop().await.unwrap();
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
