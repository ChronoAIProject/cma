use layer2_core::resource_access::*;
use std::time::Duration;

use super::*;

fn personal_policy(parent: &Sandbox) -> ResourcePolicyRecord {
    ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: None,
    }
}

async fn put_policy(store: &MongoSandboxStore, parent: &Sandbox, policy: &ResourcePolicyRecord) {
    let kind = match policy.owner {
        ResourceOwner::Person { .. } => "person",
        ResourceOwner::Organization { .. } => "organization",
    };
    store.resource_policies.replace_one(doc! { "_id": parent.id.as_str() }, doc! {
        "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner_kind": kind, "owner_subject": parent.owner.as_str(), "revision": policy.revision as i64,
        "record": crate::store::sandbox_mongo::record_bson(policy).unwrap(),
    }).upsert(true).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn policy_revision_is_fenced_without_aggregate_changes_and_retries_recover() {
    let (client, name) = database("command_admission_policy").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let original_request = request(&agent, &parent, "retained");
    let original = record(
        store
            .reserve_command(original_request.clone())
            .await
            .unwrap(),
    );
    put_policy(&store, &parent, &personal_policy(&parent)).await;
    let mut current = request(&agent, &parent, "fresh");
    assert_eq!(
        store.reserve_command(current.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        record(store.reserve_command(original_request).await.unwrap()),
        original
    );
    current.policy.policy_revision = 1;
    current.envelope.authorization.policy_revision = 17;
    let admitted = record(store.reserve_command(current).await.unwrap());
    assert_eq!(admitted.receipt.agent_order, 2);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ledger_hold_refuses_new_command_and_retains_exact_replay() {
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;

    let (client, name) = database("command_admission_ledger_hold").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let retained = request(&agent, &parent, "retained");
    let fresh = request(&agent, &parent, "fresh");
    let admitted = record(store.reserve_command(retained.clone()).await.unwrap());
    let agent_key = doc! {"_id": agent.id.as_str()};
    let parent_key = doc! {"_id": parent.id.as_str()};
    let agent_before = store.agents.find_one(agent_key.clone()).await.unwrap();
    let lane_before = store
        .agent_command_orders
        .find_one(agent_key.clone())
        .await
        .unwrap();
    let mut fields = Document::new();
    fields.insert(LEDGER_MAINTENANCE_FIELD, Bson::Null);
    store
        .sandboxes
        .update_one(parent_key.clone(), doc! {"$set": fields})
        .await
        .unwrap();
    let parent_before = store.sandboxes.find_one(parent_key.clone()).await.unwrap();

    assert_eq!(
        store.reserve_command(fresh.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        store.reserve_command(retained).await.unwrap(),
        CommandReservationResult::Existing(Box::new(admitted))
    );
    assert_eq!(
        store.agents.find_one(agent_key.clone()).await.unwrap(),
        agent_before
    );
    assert_eq!(
        store.sandboxes.find_one(parent_key).await.unwrap(),
        parent_before
    );
    assert_eq!(
        store
            .agent_command_orders
            .find_one(agent_key)
            .await
            .unwrap(),
        lane_before
    );
    assert_eq!(
        store
            .get_admitted_command(&fresh.scope(), &fresh.key_sha256)
            .await
            .unwrap(),
        None
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_kind_incarnation_and_corrupt_policy_cannot_consume_admission_order() {
    let (client, name) = database("command_admission_integrity").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let mut submission = request(&agent, &parent, "fenced");
    submission.policy.policy_revision = 1;
    let mut policy = personal_policy(&parent);
    policy.owner = ResourceOwner::Organization {
        subject: parent.owner.clone(),
    };
    policy.organization_evidence = Some(OrganizationOwnershipEvidence {
        issuer: "https://identity.test".into(),
        organization: parent.owner.clone(),
        admitted_by: ResourceActor::Human {
            subject: SubjectId::new("admin"),
        },
        verified_at: NOW.into(),
    });
    put_policy(&store, &parent, &policy).await;
    assert_eq!(
        store.reserve_command(submission.clone()).await,
        Err(CommandLedgerError::NotFound)
    );
    for corruption in 0..3 {
        let mut policy = personal_policy(&parent);
        match corruption {
            0 => policy.sandbox_created_at = TWO_MINUTES.into(),
            1 => policy.revision = 0,
            _ => {
                policy.owner = ResourceOwner::Person {
                    subject: SubjectId::new("foreign"),
                }
            }
        }
        put_policy(&store, &parent, &policy).await;
        assert_eq!(
            store.reserve_command(submission.clone()).await,
            Err(CommandLedgerError::Corrupt)
        );
    }
    put_policy(&store, &parent, &personal_policy(&parent)).await;
    submission.policy.sandbox_created_at = TWO_MINUTES.into();
    assert_eq!(
        store.reserve_command(submission.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    submission.policy.sandbox_created_at = parent.created_at.clone();
    assert_eq!(
        record(store.reserve_command(submission).await.unwrap())
            .receipt
            .agent_order,
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn response_identity_has_a_unique_index_and_duplicate_refusal_rolls_back_order() {
    let (client, name) = database("command_response_identity").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let first = record(
        store
            .reserve_command(request(&agent, &parent, "first"))
            .await
            .unwrap(),
    );
    let response_id = first.receipt.response_id.as_ref().unwrap();
    assert_eq!(
        store.get_response_command(response_id).await.unwrap(),
        Some(first.clone())
    );
    let indexes: Vec<_> = store
        .agent_commands
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(indexes
        .iter()
        .any(|index| index.keys == doc! { "response_id": 1_i32 }
            && index
                .options
                .as_ref()
                .is_some_and(|options| options.unique == Some(true))));
    let mut duplicate = request(&agent, &parent, "second");
    duplicate.response_id = Some(response_id.clone());
    assert_eq!(
        store.reserve_command(duplicate).await,
        Err(CommandLedgerError::Conflict)
    );
    assert!(store
        .get_response_command(&AgentResponseId::generate())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        record(
            store
                .reserve_command(request(&agent, &parent, "third"))
                .await
                .unwrap()
        )
        .receipt
        .agent_order,
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn membership_expiry_during_transaction_io_aborts_every_write_and_retains_old_receipts() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_admission_expiry_{suffix}");
    let app_name = format!("cma-admission-expiry-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, agent, parent) = fixture(&name, &client).await;
    let original_request = request(&agent, &parent, "retained");
    let original = record(
        store
            .reserve_command(original_request.clone())
            .await
            .unwrap(),
    );
    let before_agent = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap();
    let before_parent = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap();
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
        "data": { "failCommands": ["insert"], "appName": &app_name, "blockConnection": true, "blockTimeMS": 1000_i32 },
    }).await.unwrap();
    let mut expiring = request(&agent, &parent, "expiring");
    expiring.admission_audit = Some(super::audit_tests::personal_audit(&agent, &parent));
    expiring.authority_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(500));
    let result = store.reserve_command(expiring).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable fixture failpoint before assertions");
    assert_eq!(result, Err(CommandLedgerError::Conflict));
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap(),
        before_agent
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap(),
        before_parent
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    let mut expired_retry = original_request;
    expired_retry.authority_deadline = Some(tokio::time::Instant::now());
    assert_eq!(
        store.reserve_command(expired_retry).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        store
            .get_response_command(original.receipt.response_id.as_ref().unwrap())
            .await
            .unwrap(),
        Some(original)
    );
    assert_eq!(
        record(
            store
                .reserve_command(request(&agent, &parent, "fresh"))
                .await
                .unwrap()
        )
        .receipt
        .agent_order,
        2
    );
    client.database(&name).drop().await.unwrap();
}
