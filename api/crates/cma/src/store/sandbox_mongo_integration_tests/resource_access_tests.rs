#[path = "resource_access_tests/access_snapshot_session.rs"]
mod access_snapshot_session;
#[path = "resource_access_tests/artifacts.rs"]
mod artifacts;
#[path = "resource_access_tests/backend_free.rs"]
mod backend_free;
#[path = "resource_access_tests/hierarchy.rs"]
mod hierarchy;
#[path = "resource_access_tests/history_measurement.rs"]
mod history_measurement;
#[path = "resource_access_tests/inventory.rs"]
mod inventory;
#[path = "resource_access_tests/observation.rs"]
mod observation;
#[path = "resource_access_tests/observation_leases.rs"]
mod observation_leases;
#[path = "resource_access_tests/personal_inventory.rs"]
mod personal_inventory;
#[path = "resource_access_tests/provenance.rs"]
mod provenance;
#[path = "resource_access_tests/sandbox_inventory.rs"]
mod sandbox_inventory;
#[path = "resource_access_tests/sandbox_snapshot.rs"]
mod sandbox_snapshot;
#[path = "resource_access_tests/share_links.rs"]
mod share_links;
#[path = "resource_access_tests/share_transaction_contract.rs"]
mod share_transaction_contract;
#[path = "resource_access_tests/snapshot_reads.rs"]
mod snapshot_reads;
use layer2_core::resource_access::*;
use sha2::{Digest as _, Sha256};

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::ready_agent;

pub(in crate::store) async fn fixture(
    client: &Client,
    name: &str,
) -> (MongoSandboxStore, ResourceTarget) {
    let store = MongoSandboxStore::open(client, name).await.unwrap();
    let parent = sandbox("sbx_shares");
    let mut agent = ready_agent(&parent, "agt_share");
    agent.created_at = NOW.into();
    agent.updated_at = NOW.into();
    agent.ready_at = Some(NOW.into());
    agent.conversation.last_seq = 2;
    let workspace = ready_workspace(&parent, agent.workspace_id.as_str(), "project");
    let (mut worktree, _) = worktree_and_agent(&parent.id);
    worktree.id = agent.worktree_id.clone();
    worktree.workspace_id = workspace.id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_sandbox(parent).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    // This fixture represents a retained conversation. Normal admission only
    // accepts an empty log; install the matching retained record and log here.
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        ),
        (
            2,
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
        ),
    ] {
        store
            .agent_log
            .insert_one(
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    (store, ResourceTarget::Agent { agent_id: agent.id })
}
pub(super) fn mutation(
    snapshot: &ResourceAccessSnapshot,
    key: &str,
    mode: ShareMode,
) -> ShareMutation {
    ShareMutation {
        selector: match &snapshot.lineage.agent {
            Some(agent) => ResourceSelector::Agent {
                agent_id: agent.id.clone(),
            },
            None => ResourceSelector::Sandbox {
                sandbox_id: snapshot.lineage.sandbox.id.clone(),
            },
        },
        owner: ResourceOwner::Person {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        actor: ResourceActor::Human {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        key_sha256: format!("{:x}", Sha256::digest(key)),
        change: ShareChange::Create {
            target_org_id: SubjectId::new("org"),
            mode,
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: format!("shr_{key}"),
        at: NOW.into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replicas_recover_one_share_receipt_and_preserve_the_live_agent_and_ancestry() {
    let (client, name) = database("resource_share_replay").await;
    let (one, target) = fixture(&client, &name).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = one.resource_access_snapshot(&target).await.unwrap();
    let agent = before.lineage.agent.as_ref().unwrap();
    let logs = one.agent_log(&agent.id, 0, 50).await.unwrap();
    let first = mutation(&before, "create", ShareMode::View);
    let mut retry = first.clone();
    retry.proposed_id = "different-generated-id".into();
    let (a, b) = tokio::join!(one.mutate_share(first.clone()), two.mutate_share(retry));
    let receipt = a.unwrap();
    assert_eq!(b.unwrap(), receipt);
    assert_eq!(
        one.resource_shares.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(
        one.resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let after = one.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(after.lineage, before.lineage);
    assert_eq!(one.agent_log(&agent.id, 0, 50).await.unwrap(), logs);
    assert_eq!(after.shares, std::slice::from_ref(&receipt.share));
    assert_eq!(after.fence().policy_revision, 1);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .share_mutation_receipt(&receipt.operation_id)
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(reopened.mutate_share(first.clone()).await.unwrap(), receipt);
    let mut changed = first;
    changed.change = ShareChange::Create {
        target_org_id: SubjectId::new("other"),
        mode: ShareMode::View,
    };
    assert_eq!(
        reopened.mutate_share(changed).await,
        Err(ResourceAccessStoreError::IdempotencyConflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_changes_require_current_policy_and_share_revisions() {
    let (client, name) = database("resource_share_cas").await;
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let created = store
        .mutate_share(mutation(&before, "create", ShareMode::View))
        .await
        .unwrap();
    let current = store.resource_access_snapshot(&target).await.unwrap();
    let mut update = mutation(&current, "update", ShareMode::View);
    update.change = ShareChange::SetMode {
        share_id: created.share.id.clone(),
        mode: ShareMode::View,
    };
    update.expected_share_revision = Some(1);
    let mut revoke = mutation(&current, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: created.share.id.clone(),
    };
    revoke.expected_share_revision = Some(1);
    let (a, b) = tokio::join!(store.mutate_share(update), store.mutate_share(revoke));
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(
        matches!(a, Err(ResourceAccessStoreError::Conflict))
            || matches!(b, Err(ResourceAccessStoreError::Conflict))
    );
    assert_eq!(
        store
            .resource_share(&created.share.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store
            .mutate_share(mutation(&before, "stale", ShareMode::View))
            .await,
        Err(ResourceAccessStoreError::Conflict)
    );
    assert_eq!(
        store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn audit_insert_failure_rolls_back_grants_policy_and_resource_fences() {
    let (client, name) = database("resource_share_rollback").await;
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":before.lineage.sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    client.database(&name).run_command(doc!{"collMod":"resource_share_commands","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let request = mutation(&before, "rollback", ShareMode::View);
    assert_eq!(
        store.mutate_share(request.clone()).await,
        Err(ResourceAccessStoreError::Unavailable)
    );
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        before
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! {"_id":before.lineage.sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        parent
    );
    assert_eq!(
        store
            .resource_policies
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .resource_shares
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client
        .database(&name)
        .run_command(doc! {"collMod":"resource_share_commands","validator":{}})
        .await
        .unwrap();
    store.mutate_share(request).await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unsafe_interaction_is_refused_and_revocation_removes_its_whole_sandbox_support() {
    let (client, name) = database("resource_share_isolation").await;
    let (store, target) = fixture(&client, &name).await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(
        store
            .mutate_share(mutation(&original, "unsafe", ShareMode::OrgRoles))
            .await,
        Err(ResourceAccessStoreError::UnsafeInteractiveShare)
    );
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        original
    );
    let parent_target = ResourceTarget::Sandbox {
        sandbox_id: original.lineage.sandbox.id.clone(),
    };
    let current = store
        .resource_access_snapshot(&parent_target)
        .await
        .unwrap();
    let whole = store
        .mutate_share(mutation(&current, "whole", ShareMode::OrgRoles))
        .await
        .unwrap();
    let current = store.resource_access_snapshot(&target).await.unwrap();
    let narrow = store
        .mutate_share(mutation(&current, "narrow", ShareMode::OrgRoles))
        .await
        .unwrap();
    let current = store
        .resource_access_snapshot(&parent_target)
        .await
        .unwrap();
    let mut revoke = mutation(&current, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: whole.share.id,
    };
    revoke.expected_share_revision = Some(1);
    store.mutate_share(revoke.clone()).await.unwrap();
    store.mutate_share(revoke).await.unwrap();
    let current = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(current.shares, std::slice::from_ref(&narrow.share));
    assert_eq!(
        narrow
            .share
            .require_safe_interaction(&current.scope(&target).unwrap(), &current.shares),
        Err(AccessError::UnsafeInteractiveShare)
    );
    assert_eq!(current.lineage, original.lineage);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_policy_projection_and_torn_ownership_evidence_fail_closed() {
    let (client, name) = database("resource_share_corruption").await;
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    store
        .mutate_share(mutation(&before, "create", ShareMode::View))
        .await
        .unwrap();
    store
        .resource_policies
        .update_one(doc! {}, doc! {"$set":{"owner_kind":"organization"}})
        .await
        .unwrap();
    assert_eq!(
        store.resource_access_snapshot(&target).await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    store.resource_policies.delete_many(doc! {}).await.unwrap();
    assert_eq!(
        store.resource_access_snapshot(&target).await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lost_commit_acknowledgment_recovers_the_original_share_and_audit_exactly_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_share_commit_{suffix}");
    let app_name = format!("cma-share-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let request = mutation(&before, "ambiguous", ShareMode::View);
    admin.database("admin").run_command(doc!{"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":app_name,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous share commit"}}}).await.unwrap();
    let result = store.mutate_share(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(ResourceAccessStoreError::Unavailable));
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let recovered = store.mutate_share(request.clone()).await.unwrap();
    assert_eq!(store.mutate_share(request).await.unwrap(), recovered);
    assert_eq!(
        store
            .resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .resource_shares
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let after = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(after.lineage, before.lineage);
    assert_eq!(after.fence().policy_revision, 1);
    client.database(&name).drop().await.unwrap();
}

#[path = "resource_access_tests/mutation_contention.rs"]
mod mutation_contention;
