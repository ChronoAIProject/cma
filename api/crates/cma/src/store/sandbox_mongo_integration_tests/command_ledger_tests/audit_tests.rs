use super::*;
use layer2_core::resource_access::*;

pub(super) fn personal_audit(agent: &Agent, parent: &Sandbox) -> ResourceAuthorizationAudit {
    ResourceAuthorizationAudit {
        target: ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        },
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
        actor: ResourceActor::Human {
            subject: agent.owner.clone(),
        },
        policy_revision: 0,
        capability: ResourceCapability::Interact,
        captured_at: layer2_core::now_rfc3339(),
        observations: vec![ResourceAuthorityObservation {
            capabilities: vec![ResourceCapability::Interact],
            basis: ResourceAuthorizationBasis::PersonalOwner,
        }],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_audit_is_atomic_immutable_and_recovered_across_replicas() {
    let (client, name) = database("command_audit_admission").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut a = request(&agent, &parent, "audited");
    a.admission_audit = Some(personal_audit(&agent, &parent));
    let mut b = a.clone();
    b.envelope.command_id = AgentCommandId::generate();
    b.envelope.authorization.policy_revision = 17;
    let (left, right) = tokio::join!(one.reserve_command(a.clone()), two.reserve_command(b));
    let original = record(left.unwrap());
    assert_eq!(record(right.unwrap()), original);
    let page = PageRequest::new(20, None).unwrap();
    let audits = one
        .command_authorization_audits(&original.receipt.envelope.command_id, &page)
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(
        audits.items[0].credential_authority,
        original.receipt.envelope.authorization
    );
    assert_eq!(audits.items[0].resources[0].policy_revision, 0);
    assert_eq!(
        one.command_authorization_audits(&AgentCommandId::generate(), &page)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    drop(one);
    drop(two);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        record(reopened.reserve_command(a.clone()).await.unwrap()),
        original
    );
    assert_eq!(
        reopened
            .command_authorization_audits(&original.receipt.envelope.command_id, &page)
            .await
            .unwrap(),
        audits
    );
    assert_eq!(
        reopened
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    a.envelope.command = AgentCommand::message("changed request");
    a.envelope.request_fingerprint = a.envelope.command.fingerprint().unwrap();
    assert_eq!(
        reopened.reserve_command(a).await,
        Err(CommandLedgerError::IdempotencyConflict)
    );
    assert_eq!(
        reopened
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mismatched_admission_audit_cannot_write_and_corrupt_audit_reads_fail_closed() {
    let (client, name) = database("command_audit_corrupt").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    for field in ["actor", "target", "policy", "capability"] {
        let mut intent = request(&agent, &parent, field);
        let mut audit = personal_audit(&agent, &parent);
        match field {
            "actor" => {
                audit.actor = ResourceActor::Human {
                    subject: SubjectId::new("foreign"),
                }
            }
            "target" => {
                audit.target = ResourceTarget::Agent {
                    agent_id: layer2_core::AgentId("foreign".into()),
                }
            }
            "policy" => audit.policy_revision = 1,
            "capability" => audit.capability = ResourceCapability::ManageSandbox,
            _ => unreachable!(),
        }
        intent.admission_audit = Some(audit);
        assert_eq!(
            store.reserve_command(intent).await,
            Err(CommandLedgerError::Invalid)
        );
    }
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    let mut intent = request(&agent, &parent, "valid");
    intent.admission_audit = Some(personal_audit(&agent, &parent));
    let accepted = record(store.reserve_command(intent).await.unwrap());
    assert_eq!(accepted.receipt.agent_order, 1);
    store
        .command_authorization_audits
        .update_one(
            doc! {},
            doc! { "$set": { "record.schema_version": 99_i32 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .command_authorization_audits(
                &accepted.receipt.envelope.command_id,
                &PageRequest::new(20, None).unwrap()
            )
            .await,
        Err(CommandLedgerError::Corrupt)
    );
    // Audit corruption cannot reinterpret or replace the serialized receipt.
    assert_eq!(
        store
            .get_command(&accepted.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(accepted)
    );
    client.database(&name).drop().await.unwrap();
}
