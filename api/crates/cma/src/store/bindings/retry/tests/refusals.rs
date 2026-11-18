use super::*;
use futures_util::TryStreamExt;

#[derive(Clone, Copy)]
enum Refusal {
    Unsupported,
    Malformed,
    MissingProof,
    Protocol,
    StaleBinding,
    ChangedKey,
    ParentOwner,
    Cleanup,
    PendingConversation,
    BusyChild,
    WrongKey,
}

async fn check(case: Refusal) {
    let f = Fixture::new(true, 4).await;
    let database = f.admin.database(&f.database);
    let (collection, filter, change) = match case {
        Refusal::Unsupported => (
            "sandboxes",
            doc! {"_id":f.sandbox.id.as_str()},
            doc! {"$set":{"document_schema_version":5_i64}},
        ),
        Refusal::Malformed => (
            "sandboxes",
            doc! {"_id":f.sandbox.id.as_str()},
            doc! {"$set":{"record.work":"malformed"}},
        ),
        Refusal::MissingProof => (
            "sandboxes",
            doc! {"_id":f.sandbox.id.as_str()},
            doc! {"$unset":{"record.work.prepared_installation_digest":""}},
        ),
        Refusal::Protocol => (
            "workspaces",
            doc! {"_id":f.workspace.id.as_str()},
            doc! {"$set":{"record.work.authority_protocol":"unsupported"}},
        ),
        Refusal::StaleBinding => (
            "provider_bindings",
            doc! {"_id":&f.binding.binding_id},
            doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
        ),
        Refusal::ChangedKey => (
            "sandboxes",
            doc! {"_id":f.sandbox.id.as_str()},
            doc! {"$inc":{"record.credential.key_generation":1_i64}},
        ),
        Refusal::ParentOwner => (
            "workspaces",
            doc! {"_id":f.workspace.id.as_str()},
            doc! {"$set":{"record.owner":"replacement-owner"}},
        ),
        Refusal::Cleanup => (
            "workspaces",
            doc! {"_id":f.workspace.id.as_str()},
            doc! {"$set":{"owned_workspace_cleanup_admission_v1":{"pending":true}}},
        ),
        Refusal::PendingConversation | Refusal::BusyChild | Refusal::WrongKey => {
            ("", doc! {}, doc! {})
        }
    };
    if !collection.is_empty() {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .update_one(filter, change)
                .await
                .unwrap()
                .matched_count,
            1
        );
    }
    if matches!(case, Refusal::PendingConversation) {
        database
            .collection::<Document>("binding_conversations")
            .insert_one(doc! {
                "_id":"pending", "binding_id":&f.binding.binding_id, "pending":true,
            })
            .await
            .unwrap();
    }
    if matches!(case, Refusal::BusyChild) {
        database
            .collection::<Document>("agents")
            .insert_one(doc! {
                "_id":"busy-child", "sandbox_id":f.sandbox.id.as_str(), "work_pending":true,
            })
            .await
            .unwrap();
    }
    let before = state(&f).await;
    let wrong = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let result = f
        .controller
        .retry_recovered_binding(
            &f.binding,
            if matches!(case, Refusal::WrongKey) {
                &wrong
            } else {
                &f.key
            },
        )
        .await;
    let trace = f.release_fault().await;
    let after = state(&f).await;
    f.cleanup().await;
    assert!(result.is_err(), "invalid custody or hierarchy must refuse");
    assert!(
        after == before,
        "refusal must retain complete hierarchy and encrypted custody"
    );
    assert_eq!(trace.starts, 1, "domain refusal must not retry");
    assert_eq!(trace.commits, 0, "domain refusal must not send commit");
}

async fn state(f: &Fixture) -> Vec<Vec<Document>> {
    let mut state = Vec::new();
    for name in [
        "sandboxes",
        "workspaces",
        "provider_bindings",
        "sandbox_credentials",
        "binding_conversations",
        "agents",
        "worktrees",
    ] {
        state.push(
            f.admin
                .database(&f.database)
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    state
}

macro_rules! case {
    ($name:ident, $case:ident) => {
        #[tokio::test]
        #[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
        async fn $name() { check(Refusal::$case).await; }
    };
}
case!(unsupported_parent_schema_refuses, Unsupported);
case!(malformed_parent_refuses, Malformed);
case!(prepared_parent_without_proof_refuses, MissingProof);
case!(inconsistent_parent_protocol_refuses, Protocol);
case!(stale_binding_revision_refuses, StaleBinding);
case!(changed_key_generation_refuses, ChangedKey);
case!(replacement_parent_owner_refuses, ParentOwner);
case!(active_parent_cleanup_refuses, Cleanup);
case!(pending_conversation_refuses, PendingConversation);
case!(busy_child_refuses, BusyChild);
case!(wrong_provided_key_refuses, WrongKey);
