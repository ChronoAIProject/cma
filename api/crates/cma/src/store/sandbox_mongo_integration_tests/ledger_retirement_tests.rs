use super::*;
use crate::store::ledger_retirement::observe;
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

#[path = "../ledger_collection_mongo_tests.rs"]
mod collection;
#[path = "../ledger_collection_progress_tests.rs"]
mod collection_progress;

pub(in crate::store) fn hierarchy() -> (Sandbox, Workspace, Worktree, Agent) {
    let mut sandbox = sandbox("sbx_retirement_fixture");
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: "runtime-fixture".into(),
        runtime_version: "image-fixture".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-pvc".into(),
        storage_generation: "fixture-pvc-generation".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    let mut workspace = workspace(&sandbox, "wks_atomic", "project");
    workspace.status = WorkspaceStatus::Ready;
    workspace.work.step = WorkspaceWorkStep::None;
    let (mut worktree, mut agent) = worktree_and_agent(&sandbox.id);
    worktree.status = WorktreeStatus::Removed;
    agent.status = AgentStatus::Deleted;
    agent.deleted_at = Some(NOW.into());
    agent.work.step = AgentWorkStep::None;
    (sandbox, workspace, worktree, agent)
}

fn documents() -> Vec<(&'static str, Document)> {
    let (sandbox, workspace, worktree, agent) = hierarchy();
    vec![
        (
            SANDBOXES,
            crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap(),
        ),
        (
            WORKSPACES,
            crate::store::workspaces::workspace_document(&workspace).unwrap(),
        ),
        (
            WORKTREES,
            crate::store::worktrees::worktree_document(&worktree).unwrap(),
        ),
        (
            AGENTS,
            crate::store::agents::agent_document(&agent).unwrap(),
        ),
    ]
}

async fn install(database: &mongodb::Database, documents: &[(&str, Document)]) {
    for (collection, document) in documents {
        database
            .collection::<Document>(collection)
            .insert_one(document)
            .await
            .unwrap();
    }
}

async fn unchanged(database: &mongodb::Database, documents: &[(&str, Document)]) {
    let mut collections = database.list_collection_names().await.unwrap();
    collections.sort();
    let mut expected: Vec<_> = documents.iter().map(|(name, _)| name.to_string()).collect();
    expected.sort();
    assert_eq!(
        collections, expected,
        "observation must not initialize stores"
    );
    for (name, document) in documents {
        let collection = database.collection::<Document>(name);
        assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
        assert_eq!(
            collection.find_one(doc! {}).await.unwrap().unwrap(),
            *document
        );
        assert_eq!(collection.list_index_names().await.unwrap(), ["_id_"]);
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn snapshot_observes_settled_deletion_without_writes_or_startup_ddl() {
    let (client, name) = database("retirement_observation").await;
    let db = client.database(&name);
    let documents = documents();
    install(&db, &documents).await;
    let result = observe(&client, &db, "sbx_retirement_fixture")
        .await
        .unwrap();
    assert!(result.contains_agent("agt_atomic"));
    assert!(result.contains_worktree("project", "wt-1"));
    assert_eq!(result.agents_observed, 1);
    let again = observe(&client, &db, "sbx_retirement_fixture")
        .await
        .unwrap();
    assert_eq!(result.source_sha256, again.source_sha256);
    unchanged(&db, &documents).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn snapshot_refuses_projection_drift_corruption_missing_parent_and_future_fields() {
    let (client, name) = database("retirement_refusal").await;
    for case in 0..6 {
        let db = client.database(&format!("{name}_{case}"));
        let mut documents = documents();
        match case {
            0 => {
                documents[3].1.insert("sandbox_id", "foreign");
            }
            1 => {
                documents[3].1.insert("record", "corrupt");
            }
            2 => {
                documents.remove(1);
            }
            3 => {
                documents[3]
                    .1
                    .get_document_mut("record")
                    .unwrap()
                    .insert("future_field", true);
            }
            _ => {
                let index = if case == 4 { 3 } else { 2 };
                documents[index].1.insert("sandbox_id", "foreign");
                documents[index]
                    .1
                    .get_document_mut("record")
                    .unwrap()
                    .insert("sandbox_id", "foreign");
            }
        }
        install(&db, &documents).await;
        assert!(
            observe(&client, &db, "sbx_retirement_fixture")
                .await
                .is_err(),
            "case {case}"
        );
        unchanged(&db, &documents).await;
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn snapshot_preserves_live_and_missing_agents() {
    let (client, name) = database("retirement_live").await;
    for missing in [false, true] {
        let db = client.database(&format!("{name}_{missing}"));
        let mut documents = documents();
        if missing {
            documents.pop();
        } else {
            let (_, _, _, mut agent) = hierarchy();
            agent.status = AgentStatus::Ready;
            agent.conversation.conversation_id = Some("fixture-conversation".into());
            agent.conversation.runtime_generation = Some(1);
            agent.deleted_at = None;
            let mut document = crate::store::agents::agent_document(&agent).unwrap();
            document.insert("_id", agent.id.as_str());
            documents[3].1 = document;
        }
        install(&db, &documents).await;
        let result = observe(&client, &db, "sbx_retirement_fixture")
            .await
            .unwrap();
        assert!(result.deleted_agents.is_empty());
        assert!(result.removed_worktrees.is_empty());
        unchanged(&db, &documents).await;
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn snapshot_refuses_quarantined_deletion_without_changing_evidence() {
    let (client, name) = database("retirement_quarantine").await;
    for (case, marker) in [
        Bson::Document(doc! {
            "format": 1_i64,
            "reason": "integrity_validation_failed",
            "observed_head": 0_i64,
        }),
        Bson::Null,
        Bson::Document(doc! {"format": 99_i64}),
    ]
    .into_iter()
    .enumerate()
    {
        let db = client.database(&format!("{name}_{case}"));
        let mut documents = documents();
        documents[3]
            .1
            .insert(crate::store::agent_log_integrity::QUARANTINE_FIELD, marker);
        install(&db, &documents).await;
        assert!(observe(&client, &db, "sbx_retirement_fixture")
            .await
            .is_err());
        unchanged(&db, &documents).await;
        db.drop().await.unwrap();
    }
}
