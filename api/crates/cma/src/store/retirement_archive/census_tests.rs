use super::super::{
    census,
    census_catalog::{Policy, AUDIT},
};
use super::*;
use layer2_core::{budget::RequestBudget, sandbox::store::AgentStore};

#[path = "census_budget_tests.rs"]
mod budget;
#[path = "census_test_support.rs"]
mod support;
pub(in crate::store) use support::prepare_source;

#[path = "census_counter_tests.rs"]
mod counters;

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
async fn populated_observer_epochs_stay_in_place_and_outside_sandbox_capture_and_restore() {
    let (client, name, ordinary, record) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let agent = ordinary
        .get_agent(&record.requester.agent_id)
        .await
        .unwrap()
        .unwrap();
    let database = client.database(&name);
    let (temp, store, request, bytes) =
        support::prepare_source(client, database.clone(), agent.sandbox_id.as_str()).await;
    let collection = crate::store::agent_observation::COLLECTION;
    // Even the captured Agent's live lease is independent of Sandbox ownership.
    // Expired watermarks also remain present without blocking capture.
    for (id, expires) in [
        (
            agent.id.as_str(),
            mongodb::bson::DateTime::now().timestamp_millis() + 15_000,
        ),
        ("agt_unrelated_expired", 0),
    ] {
        database
            .collection::<Document>(collection)
            .insert_one(doc! {
                "_id": id, "document_schema_version": 1_i64,
                "created_at": &agent.created_at, "instance_id": "read-owner",
                "epoch": 7_i64, "expires_at": mongodb::bson::DateTime::from_millis(expires),
                "next_refresh_at": mongodb::bson::DateTime::from_millis(expires), "round": "",
            })
            .await
            .unwrap();
    }
    let epochs = support::inventory(&database, &[collection]).await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let receipt = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let manifest = verify::load(&custody, &receipt, &request, &bytes)
        .await
        .unwrap();
    let archived = manifest
        .database
        .iter()
        .find(|row| row.collection == collection)
        .unwrap();
    assert_eq!(archived.documents, 0);
    assert_eq!(archived.bytes, 0);
    assert!(archived.objects.is_empty());
    let restore = temp.0.join("restore");
    verify::restore(&custody, &receipt, &request, &bytes, &restore)
        .await
        .unwrap();
    assert!(
        std::fs::read(restore.join("database").join(format!("{collection}.bson")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(support::inventory(&database, &[collection]).await, epochs);
    database.drop().await.unwrap();
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

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn unsupported_budget_keys_and_missing_nested_ownership_refuse_certification() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    for key in [
        "unqualified-ledger",
        "request:agt_missing:call",
        "request-turn:agt_missing:invalid",
    ] {
        store
            .database
            .collection::<Document>("model_budget_reservations")
            .insert_one(doc! {
                "_id":key, "document_schema_version":1_i64, "record":{}
            })
            .await
            .unwrap();
        assert!(
            export::publish(&store, &request, &bytes, &Custody::default(), None)
                .await
                .is_err()
        );
        assert!(store.manifest(&request, &bytes).await.is_err());
        store
            .database
            .collection::<Document>("model_budget_reservations")
            .drop()
            .await
            .unwrap();
    }
    assert!(
        read_sandbox(&store.inspect(&request.sandbox_id).await.unwrap())
            .unwrap()
            .retirement_archive_pending()
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn real_activity_store_keeps_prefixed_and_concrete_path_resource_history() {
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let (temp, store, request, bytes) = fixture().await;
    let activity = crate::store::activity::MongoActivityStore::open(
        &store.client,
        store.database.name(),
        std::time::Duration::from_secs(86_400),
    )
    .await
    .unwrap();
    for target in [
        request.sandbox_id.clone(),
        format!("sandbox:{}", request.sandbox_id),
        format!("/api/v1/sandboxes/{}/workspaces", request.sandbox_id),
        format!("/api/v1/admin/sandboxes/{}", request.sandbox_id),
    ] {
        activity
            .append(
                &ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
                    .on(ActivityTarget::Resource(target)),
            )
            .await
            .unwrap();
    }
    let expected = support::inventory(&store.database, &["activity"]).await;
    for target in [
        "/api/v1/sandboxes/{id}".to_string(),
        format!("sandbox:{}_other", request.sandbox_id),
        format!("/api/v1/sandboxes/{}_other", request.sandbox_id),
    ] {
        activity
            .append(
                &ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
                    .on(ActivityTarget::Resource(target)),
            )
            .await
            .unwrap();
    }
    let encoded = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded).on(
        ActivityTarget::Resource(format!(
            "/api/v1/sandboxes/{}",
            request.sandbox_id.replace('_', "%5f")
        )),
    );
    activity.append(&encoded).await.unwrap();
    store.admit(&request, &bytes).await.unwrap();
    assert!(
        export::publish(&store, &request, &bytes, &Custody::default(), None)
            .await
            .is_err()
    );
    assert!(store.manifest(&request, &bytes).await.is_err());
    // Remove only this deliberately ambiguous test fixture before the success
    // branch. Production refusal requires a reviewed reader, never row removal.
    store
        .database
        .collection::<Document>("activity")
        .delete_one(doc! {"_id":&encoded.id.0})
        .await
        .unwrap();
    support::roundtrip((temp, store, request, bytes), expected).await;
}

#[path = "census_document_tests.rs"]
mod documents;
