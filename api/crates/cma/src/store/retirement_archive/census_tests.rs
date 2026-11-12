use super::super::{
    census,
    census_catalog::{Policy, AUDIT},
};
use super::*;
use layer2_core::{budget::RequestBudget, sandbox::store::AgentStore};

#[path = "census_test_support.rs"]
mod support;

#[test]
fn every_declared_namespace_has_exactly_one_explicit_policy() {
    census::validate_catalog().unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn full_capture_restores_real_access_request_consent_and_excludes_identity_collisions() {
    let (client, name, _, record) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let database = client.database(&name);
    let fixture = support::prepare_source(
        client,
        database.clone(),
        record.requester.sandbox_id.as_str(),
    )
    .await;
    let expected = support::inventory(
        &database,
        &[
            "agent_access_requests",
            "agent_creation_provenance",
            "agent_log",
            "agents",
            "workspaces",
            "worktrees",
        ],
    )
    .await;
    // The old generic _id clause would sweep in this unrelated namespace record.
    database
        .collection::<Document>("agent_access_requests")
        .insert_one(doc! {
            "_id":record.requester.agent_id.as_str(), "document_schema_version":1_i64,
            "record":{"requester":{"sandbox_id":"sbx_unrelated","agent_id":"agt_unrelated"}}
        })
        .await
        .unwrap();
    support::roundtrip(fixture, expected).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn full_capture_restores_real_request_turn_and_command_budget_ledgers_and_audits() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("archive_budget_{}", uuid::Uuid::new_v4().simple());
    let (ordinary, scope, model) = crate::store::sandbox_mongo_integration_tests::command_ledger_tests::execution_tests::request_budget::completed(&client, &name).await;
    ordinary
        .reserve_request(&scope, &model)
        .await
        .unwrap()
        .unwrap();
    let agent = ordinary.get_agent(&scope.agent_id).await.unwrap().unwrap();
    let database = client.database(&name);
    // Four resource records: command reservation, turn reservation, nested request,
    // and the request-turn ledger whose only ownership is in its composite key.
    assert_eq!(
        database
            .collection::<Document>("model_budget_reservations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        5
    );
    let fixture =
        support::prepare_source(client, database.clone(), agent.sandbox_id.as_str()).await;
    let expected = support::inventory(
        &database,
        &[
            "model_budget_reservations",
            "agent_commands",
            "command_authorization_audits",
            "agent_log",
        ],
    )
    .await;
    for key in [
        "request-turn:agt_unrelated:1",
        "request-turn:agt_commands_suffix:1",
    ] {
        database.collection::<Document>("model_budget_reservations").insert_one(doc! {
            "_id":key,"document_schema_version":1_i64,"record":{"spent":0_i64,"reserved":1_i64,"calls":1_i64,"bound_violated":false}
        }).await.unwrap();
    }
    support::roundtrip(fixture, expected).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn every_unsupported_nonempty_namespace_refuses_and_never_copies_unrelated_auth() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    let closure = census::identities(&store.database, &request.sandbox_id, &mut session)
        .await
        .unwrap();
    for (name, policy) in AUDIT {
        if !matches!(policy, Policy::Empty) {
            continue;
        }
        store
            .database
            .collection::<Document>(name)
            .insert_one(doc! {"_id":"unresolved-reference"})
            .await
            .unwrap();
        // A new snapshot must observe the unsupported row, including names with
        // no surviving decoder and indirect/composite/encrypted ownership.
        let mut snapshot = store.client.start_session().snapshot(true).await.unwrap();
        assert!(
            census::selection(&store.database, name, &closure, &mut snapshot)
                .await
                .is_err(),
            "{name}"
        );
        store
            .database
            .collection::<Document>(name)
            .drop()
            .await
            .unwrap();
    }
    store
        .database
        .collection::<Document>("auth_subjects")
        .insert_one(doc! {
            "_id":&request.sandbox_id, "opaque":"unrelated authentication custody"
        })
        .await
        .unwrap();
    let manifest = export::build(&store, &request, &bytes, &Custody::default(), None)
        .await
        .unwrap();
    assert_eq!(
        manifest
            .database
            .iter()
            .find(|r| r.collection == "auth_subjects")
            .unwrap()
            .documents,
        0
    );
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    assert!(store.manifest(&request, &bytes).await.is_err());
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn unknown_ownership_schema_cannot_hide_behind_an_unmatched_selector() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    for version in [Bson::Int64(2), Bson::Null] {
        store
            .database
            .collection::<Document>("agent_access_requests")
            .insert_one(doc! {
                "_id":"future-request", "document_schema_version":version,
                "future_ownership":{"parent":&request.sandbox_id}
            })
            .await
            .unwrap();
        assert!(
            export::publish(&store, &request, &bytes, &Custody::default(), None)
                .await
                .is_err()
        );
        assert!(store.manifest(&request, &bytes).await.is_err());
        assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
        store
            .database
            .collection::<Document>("agent_access_requests")
            .drop()
            .await
            .unwrap();
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn retained_consent_and_provenance_keep_removed_agent_history_in_the_archive() {
    let (client, name, _, record) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let database = client.database(&name);
    let fixture = support::prepare_source(
        client,
        database.clone(),
        record.requester.sandbox_id.as_str(),
    )
    .await;
    let expected = support::inventory(
        &database,
        &[
            "agent_access_requests",
            "agent_creation_provenance",
            "agent_log",
        ],
    )
    .await;
    // Historical records are authoritative after ordinary live hierarchy removal.
    for name in ["agents", "worktrees", "workspaces"] {
        database.collection::<Document>(name).drop().await.unwrap();
    }
    support::roundtrip(fixture, expected).await;
}
