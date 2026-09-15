use super::*;
use layer2_core::bridge::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::store::AgentLogStore;
pub(in crate::store::bridge_invokes) mod fixtures;
use fixtures::{caller, Fixture};
mod authority;
pub(in crate::store) mod channel_admission;
mod enable;
mod fixed_allocation;
mod generations;
mod issue_fence;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn explicit_issue_replay_restart_and_revoke_preserve_retained_context_and_actual_actor() {
    let f = Fixture::new().await;
    let target = ResourceTarget::Agent {
        agent_id: f.target.agent_id.clone(),
    };
    let before = f.resources.resource_access_snapshot(&target).await.unwrap();
    let logs = f
        .resources
        .agent_log(&f.target.agent_id, 0, 50)
        .await
        .unwrap();
    let request = f.issue("consent");
    let context = caller();
    let (a, b) = tokio::join!(
        f.service.issue(&context, None, request.clone()),
        f.service.issue(&context, None, request.clone())
    );
    let receipt = a.unwrap();
    assert_eq!(b.unwrap(), receipt);
    assert_eq!(receipt.actor, context.actor);
    assert_eq!(receipt.result.issued_by, context.actor);
    assert_eq!(receipt.result.target.ids, f.target);
    assert_eq!(
        receipt.result.consent,
        BridgeSharedContextConsent::UseRetainedAgentContext
    );
    let reopened =
        MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
            .await
            .unwrap();
    assert_eq!(
        reopened.receipt(&receipt.id).await.unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        reopened
            .grant(&receipt.result.id)
            .await
            .unwrap()
            .unwrap()
            .config,
        f.bridge.current().unwrap().config
    );
    let mut changed = request.clone();
    changed.expected_bridge_revision += 1;
    assert_eq!(
        f.service.issue(&context, None, changed).await,
        Err(BridgeInvokeError::IdempotencyConflict)
    );
    let proof = f
        .service
        .authorize(f.request(
            &receipt.result,
            BridgeInvokeEffect::ExecuteCommand {
                command_id: layer2_core::ids::AgentCommandId("command-one".into()),
            },
        ))
        .await
        .unwrap();
    f.effect(&proof).await.unwrap();
    let revoked = f
        .service
        .revoke(&context, None, f.revoke(&receipt.result, "revoke"))
        .await
        .unwrap();
    assert_eq!(
        f.service
            .revoke(&context, None, f.revoke(&receipt.result, "revoke"))
            .await
            .unwrap(),
        revoked
    );
    assert_eq!(f.effect(&proof).await, Err(BridgeInvokeError::Revoked));
    assert_eq!(
        f.service.issue(&context, None, request).await.unwrap(),
        receipt
    );
    assert_eq!(f.store.grants.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(f.store.audits.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(f.store.receipts.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        f.resources.resource_access_snapshot(&target).await.unwrap(),
        before
    );
    assert_eq!(
        f.resources
            .agent_log(&f.target.agent_id, 0, 50)
            .await
            .unwrap(),
        logs
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn proof_cannot_cross_recreated_target_or_revoked_custody_and_revocation_remains_available() {
    use crate::bridge::custody::BridgeCredentialStore;
    let f = Fixture::new().await;
    let grant = f
        .service
        .issue(&caller(), None, f.issue("consent"))
        .await
        .unwrap()
        .result;
    let proof = f
        .service
        .authorize(f.request(
            &grant,
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: "inbox-one".into(),
            },
        ))
        .await
        .unwrap();
    f.resources
        .agents
        .update_one(
            doc! {"_id":f.target.agent_id.as_str()},
            doc! {"$set":{"record.creation_fingerprint":"d".repeat(64),"creation_fingerprint":"d".repeat(64)}},
        )
        .await
        .unwrap();
    assert_eq!(
        f.effect(&proof).await,
        Err(BridgeInvokeError::TargetMismatch)
    );
    f.resources
        .agents
        .update_one(
            doc! {"_id":f.target.agent_id.as_str()},
            doc! {"$set":{"record.creation_fingerprint":&grant.target.agent_creation_fingerprint,"creation_fingerprint":&grant.target.agent_creation_fingerprint}},
        )
        .await
        .unwrap();
    let management = f
        .definitions
        .management_lease(&caller(), None, &f.bridge.id, f.bridge.revision)
        .await
        .unwrap();
    f.bridges
        .credentials
        .revoke(
            &management,
            &f.bridge.current().unwrap().config.connector.custody,
            1,
            &"a".repeat(64),
        )
        .await
        .unwrap();
    assert_eq!(
        f.effect(&proof).await,
        Err(BridgeInvokeError::Bridge(BridgeError::ConnectionRequired(
            BridgeSetupBlock::CustodyUnavailable
        )))
    );
    f.service
        .revoke(&caller(), None, f.revoke(&grant, "revoke"))
        .await
        .unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn concurrent_revocation_has_one_cas_winner_and_audit_failure_rolls_back_every_write() {
    let f = Fixture::new().await;
    let grant = f
        .service
        .issue(&caller(), None, f.issue("consent"))
        .await
        .unwrap()
        .result;
    f.client.database(&f.database).run_command(doc!{"collMod":"bridge_invoke_audits","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert_eq!(
        f.service
            .revoke(&caller(), None, f.revoke(&grant, "rollback"))
            .await,
        Err(BridgeInvokeError::Unavailable)
    );
    assert_eq!(f.store.grant(&grant.id).await.unwrap().unwrap().revision, 1);
    assert_eq!(f.store.receipts.count_documents(doc! {}).await.unwrap(), 1);
    f.client
        .database(&f.database)
        .run_command(doc! {"collMod":"bridge_invoke_audits","validator":{}})
        .await
        .unwrap();
    let context = caller();
    let (a, b) = tokio::join!(
        f.service.revoke(&context, None, f.revoke(&grant, "a")),
        f.service.revoke(&context, None, f.revoke(&grant, "b"))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(a == Err(BridgeInvokeError::Conflict) || b == Err(BridgeInvokeError::Conflict));
    assert_eq!(f.store.audits.count_documents(doc! {}).await.unwrap(), 2);
    f.close().await;
}
