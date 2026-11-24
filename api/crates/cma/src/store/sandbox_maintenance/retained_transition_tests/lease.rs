use super::*;
use layer2_core::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxWorkerStore,
};
use layer2_core::sandbox::{RuntimeBinding, SandboxEventKind};
use std::time::{Duration, SystemTime};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_runtime_grant_expired_legacy_claim_cannot_reserve_or_publish() {
    let (client, name, store) = database().await;
    let db = client.database(&name);
    let (sandbox, grant) = retained_fixture(&store, "retained-legacy-claim").await;
    let request = &grant.request;
    CanaryPolicy::configure_retained(
        &db,
        0,
        true,
        grant.catalog_sha256.clone(),
        grant.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    store
        .issue_retained_runtime_grant(&db, &grant)
        .await
        .unwrap();
    let receipt = store
        .redeem_retained_runtime_grant(
            &db,
            request.clone(),
            Some(RetainedRuntimeGrantPolicy {
                release_key: &grant.release_key,
                catalog_sha256: &grant.catalog_sha256,
                owner_scope_sha256: &grant.owner_scope_sha256,
                target_image: &request.target_image,
                target_config_digest: &request.target_config_digest,
            }),
        )
        .await
        .unwrap();

    for phase in [
        Phase::Verify,
        Phase::Reserve,
        Phase::Bind,
        Phase::Credential,
        Phase::Home,
        Phase::Reopen,
        Phase::Queue,
        Phase::Publish,
    ] {
        if matches!(phase, Phase::Reserve | Phase::Publish) {
            let stale = mutation(&store, &receipt, phase, "expired", 3).await;
            let before = rows(&store, &sandbox).await;
            tokio::time::sleep(Duration::from_secs(4)).await;
            assert_eq!(
                store.commit_sandbox_lifecycle(stale).await,
                Err(SandboxStoreError::Conflict),
            );
            assert_eq!(rows(&store, &sandbox).await, before);
        }
        let live = mutation(&store, &receipt, phase, "successor", 30).await;
        store.commit_sandbox_lifecycle(live).await.unwrap();
    }

    let published = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(published.status, SandboxStatus::Pausing);
    assert_eq!(
        published.runtime.unwrap().provision_generation,
        Some(receipt.successor_generation)
    );
    assert_eq!(published.lifecycle.home, sandbox.lifecycle.home);
    db.drop().await.unwrap();
}

#[derive(Clone, Copy)]
enum Phase {
    Verify,
    Reserve,
    Bind,
    Credential,
    Home,
    Reopen,
    Queue,
    Publish,
}

async fn mutation(
    store: &MongoSandboxStore,
    receipt: &RetainedRuntimeTransition,
    phase: Phase,
    holder: &str,
    lease_seconds: u64,
) -> SandboxLifecycleMutation {
    let instant = SystemTime::now();
    let now = layer2_core::rfc3339_at(instant);
    let claimed = store
        .claim_sandbox_work(
            &receipt.request.sandbox_id,
            holder,
            now.clone(),
            layer2_core::rfc3339_at(instant + Duration::from_secs(lease_seconds)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(claimed.work.authority_protocol.is_none());
    let lease = claimed.work.lease.as_ref().unwrap();
    let mut next = claimed.clone();
    let at = now.as_str();
    let fence = lease.fence;
    match phase {
        Phase::Verify => next.complete_lifecycle_home_verification(
            claimed.retained_home_recovery().unwrap(),
            holder,
            fence,
            at.into(),
        ),
        Phase::Reserve => next
            .reserve_lifecycle_runtime_generation(
                receipt.successor_generation,
                holder,
                fence,
                at.into(),
            )
            .map(|_| ()),
        Phase::Bind => {
            let prior = claimed.lifecycle.suspended_runtime.as_ref().unwrap();
            let boot_id = "a".repeat(32);
            let observation = serde_json::from_value(serde_json::json!({
                "legacy_journal_recovery_enabled": receipt.request.journal_recovery,
                "runtime": {
                    "sandbox_id": claimed.id,
                    "generation": receipt.successor_generation,
                    "home_identity": receipt.request.predecessor.home_identity,
                    "boot_id": boot_id,
                    "installation_ready": true,
                    "credential_ready": false,
                    "restore_pending": false,
                    "agent_processes": 0,
                },
            }))
            .unwrap();
            next.bind_retained_lifecycle_runtime(
                RuntimeBinding {
                    runtime_id: prior.runtime_id.clone(),
                    runtime_version: receipt.request.target_image.clone(),
                    boot_id: Some(boot_id),
                    provision_generation: Some(receipt.successor_generation),
                    bound_at: at.into(),
                },
                observation,
                holder,
                fence,
                at.into(),
            )
        }
        Phase::Credential => next.complete_lifecycle_credential_delivery(holder, fence, at.into()),
        Phase::Home => next.complete_lifecycle_home_restore(holder, fence, at.into()),
        Phase::Reopen => next.complete_lifecycle_conversation_reopen(holder, fence, at.into()),
        Phase::Queue => next.complete_lifecycle_queued_message_delivery(holder, fence, at.into()),
        Phase::Publish => next.publish_lifecycle_active(at.into(), holder, fence, at.into()),
    }
    .unwrap();
    let event = if next.status != claimed.status {
        SandboxEventKind::StatusChanged {
            from: claimed.status,
            to: next.status,
        }
    } else {
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }
    };
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&claimed),
        sandbox: next,
        agents: Vec::new(),
        agent_scope: match phase {
            Phase::Reopen | Phase::Queue => SandboxLifecycleAgentScope::AllLive,
            _ => SandboxLifecycleAgentScope::Touched,
        },
        appended_at: now,
        events: vec![event],
    }
}
