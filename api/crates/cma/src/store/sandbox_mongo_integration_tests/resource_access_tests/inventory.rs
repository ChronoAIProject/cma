use super::*;
use layer2_core::sandbox::store::AgentProvenanceInventoryStore;
use layer2_core::sandbox::AgentListQuery;
use mongodb::event::{cmap::CmapEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::sync::atomic::{AtomicU64, Ordering};

async fn add_agent(store: &MongoSandboxStore, id: &str) -> ResourceTarget {
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: AgentId("agt_share".into()),
        })
        .await
        .unwrap();
    let mut agent = snapshot.lineage.agent.unwrap();
    let mut worktree = snapshot.lineage.worktree.unwrap();
    agent.id = AgentId(id.into());
    agent.worktree_id = WorktreeId(format!("wtr_{id}"));
    agent.creation_key = format!("fixture-{id}");
    worktree.id = agent.worktree_id.clone();
    worktree.name = format!("wt-{id}");
    worktree.path = format!("/home/cma/fixture/{id}");
    worktree.branch = format!("cma/{id}");
    worktree.agent_id = Some(agent.id.clone());
    store.create_worktree(worktree).await.unwrap();
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
    ResourceTarget::Agent { agent_id: agent.id }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_current_snapshots_reduce_connection_checkouts() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let checkouts = Arc::new(AtomicU64::new(0));
    let captured = checkouts.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            captured.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_inventory_batch_{}", uuid::Uuid::new_v4().simple());
    let (store, target) = fixture(&client, &name).await;
    let targets = vec![target; 16];

    let before_individual = checkouts.load(Ordering::SeqCst);
    for target in &targets {
        store.resource_access_snapshot(target).await.unwrap();
    }
    let individual = checkouts.load(Ordering::SeqCst) - before_individual;

    let before_batch = checkouts.load(Ordering::SeqCst);
    store.resource_access_snapshots(&targets).await.unwrap();
    let batched = checkouts.load(Ordering::SeqCst) - before_batch;

    assert!(
        batched < individual,
        "batched current reads should use fewer checkouts: individual={individual}, batched={batched}"
    );
    client.database(&name).drop().await.unwrap();
}

fn ids(values: &[ResourceAccessSnapshot]) -> Vec<&str> {
    values
        .iter()
        .map(|value| value.lineage.agent.as_ref().unwrap().id.as_str())
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_inventory_filters_before_cursor_paging_and_deduplicates_overlap() {
    let (client, name) = database("organization_inventory").await;
    let (store, target) = fixture(&client, &name).await;
    for id in ["a-private", "b-other-org", "z-shared"] {
        add_agent(&store, id).await;
    }
    let before = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage;
    let logs = store
        .agent_log(&AgentId("agt_share".into()), 0, 20)
        .await
        .unwrap();
    for (id, org) in [
        ("agt_share", "org"),
        ("z-shared", "org"),
        ("b-other-org", "other-org"),
    ] {
        let snapshot = store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: AgentId(id.into()),
            })
            .await
            .unwrap();
        let mut command = mutation(&snapshot, id, ShareMode::View);
        command.change = ShareChange::Create {
            target_org_id: SubjectId::new(org),
            mode: ShareMode::View,
        };
        store.mutate_share(command).await.unwrap();
    }
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    store
        .mutate_share(mutation(&snapshot, "overlap", ShareMode::View))
        .await
        .unwrap();
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let page = two
        .organization_agents(
            &SubjectId::new("org"),
            &AgentListQuery::default(),
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&page), ["agt_share", "z-shared"]); // Authorized lookahead is retained internally.
    let creation = two
        .scan_agent_provenance_inventory(
            &ResourceOwner::Organization {
                subject: SubjectId::new("org"),
            },
            &AgentListQuery::default(),
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&creation
            .into_iter()
            .map(|row| row.provenance.resource)
            .collect::<Vec<_>>()),
        ["agt_share", "z-shared"]
    );
    let last = two
        .organization_agents(
            &SubjectId::new("org"),
            &AgentListQuery::default(),
            &PageRequest::new(1, Some("agt_share".into())).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&last), ["z-shared"]);
    let other = two
        .organization_agents(
            &SubjectId::new("other-org"),
            &AgentListQuery::default(),
            &PageRequest::new(200, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&other), ["b-other-org"]);
    let root = ResourceTarget::Sandbox {
        sandbox_id: before.sandbox.id.clone(),
    };
    let snapshot = store.resource_access_snapshot(&root).await.unwrap();
    store
        .mutate_share(mutation(&snapshot, "whole-sandbox", ShareMode::View))
        .await
        .unwrap();
    let all = two
        .organization_agents(
            &SubjectId::new("org"),
            &AgentListQuery::default(),
            &PageRequest::new(200, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&all),
        ["a-private", "agt_share", "b-other-org", "z-shared"]
    );
    let creation = two
        .scan_agent_provenance_inventory(
            &ResourceOwner::Organization {
                subject: SubjectId::new("org"),
            },
            &AgentListQuery::default(),
            &PageRequest::new(200, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&creation
            .into_iter()
            .map(|row| row.provenance.resource)
            .collect::<Vec<_>>()),
        ids(&all)
    );
    let empty = two
        .organization_agents(
            &SubjectId::new("org"),
            &AgentListQuery {
                status: Some(AgentStatus::Paused),
                ..Default::default()
            },
            &PageRequest::new(200, None).unwrap(),
        )
        .await
        .unwrap();
    assert!(empty.is_empty());
    assert_eq!(
        store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before
    );
    assert_eq!(
        store
            .agent_log(&AgentId("agt_share".into()), 0, 20)
            .await
            .unwrap(),
        logs
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_inventory_requires_explicit_valid_owner_sidecar() {
    let (client, name) = database("organization_inventory_owner").await;
    let (store, target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let root = snapshot.lineage.sandbox;
    let query = AgentListQuery::default();
    let page = PageRequest::new(200, None).unwrap();
    assert!(store
        .organization_agents(&root.owner, &query, &page)
        .await
        .unwrap()
        .is_empty());
    let policy = ResourcePolicyRecord {
        sandbox_id: root.id.clone(),
        sandbox_created_at: root.created_at,
        owner: ResourceOwner::Organization {
            subject: root.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: root.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("fixture-admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    store
        .resource_policies
        .insert_one(doc! {"_id":root.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"owner_kind":"organization","owner_subject":root.owner.as_str(),"revision":1_i64,"record":crate::store::sandbox_mongo::record_bson(&policy).unwrap()})
        .await
        .unwrap();
    assert_eq!(
        ids(&store
            .organization_agents(&root.owner, &query, &page)
            .await
            .unwrap()),
        ["agt_share"]
    );
    store
        .resource_policies
        .update_one(
            doc! {"_id":root.id.as_str()},
            doc! {"$set":{"record.sandbox_created_at":"2020-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.organization_agents(&root.owner, &query, &page).await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revoked_and_corrupt_grant_indexes_cannot_supply_inventory_rows() {
    let (client, name) = database("organization_inventory_revocation").await;
    let (store, target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let share = store
        .mutate_share(mutation(&snapshot, "grant", ShareMode::View))
        .await
        .unwrap()
        .share;
    let query = AgentListQuery::default();
    let page = PageRequest::new(200, None).unwrap();
    assert_eq!(
        ids(&store
            .organization_agents(&SubjectId::new("org"), &query, &page)
            .await
            .unwrap()),
        ["agt_share"]
    );
    store
        .resource_shares
        .update_one(
            doc! {"_id":&share.id},
            doc! {"$set":{"record.target_org_id":"another-org"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .organization_agents(&SubjectId::new("org"), &query, &page)
            .await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    store
        .resource_shares
        .update_one(
            doc! {"_id":&share.id},
            doc! {"$set":{"record.target_org_id":"org"}},
        )
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let mut revoke = mutation(&snapshot, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke { share_id: share.id };
    revoke.expected_share_revision = Some(share.revision);
    store.mutate_share(revoke).await.unwrap();
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(two
        .organization_agents(&SubjectId::new("org"), &query, &page)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

mod shared_inventory_tests;
