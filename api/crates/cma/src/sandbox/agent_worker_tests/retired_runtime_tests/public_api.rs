use super::*;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    Authorization, Capability, OperationClaim, OperationReservation, OperationStore, StoreError,
};
use tower::ServiceExt;

struct Operations;
#[async_trait::async_trait]
impl OperationStore for Operations {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        Ok(OperationReservation::Acquired)
    }
    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }
}

fn router(harness: &Harness) -> Router {
    crate::projections::AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        harness.store.clone(),
        Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default()),
        harness.registry.clone(),
        Arc::new(crate::projections::UnavailableSandboxResources),
        RuntimeCapabilities::new([RuntimeCapability::Conversation]),
    )
    .router(Arc::new(Operations))
}

async fn call(app: &Router, agent: &Agent, method: Method, suffix: &str) -> StatusCode {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/agents/{}{suffix}", agent.id))
        .header("content-type", "application/json")
        .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
        .body(Body::from("{}"))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(agent.owner.clone()));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn public_owner_stop_and_delete_settle_only_after_provider_absence() {
    for state in ["absent", "present", "unavailable"] {
        let harness = Harness::with_generation(Arc::new(EmptyAgentSkillResolver), 4).await;
        let admitted = harness.admit("public-missing-runtime", Vec::new()).await;
        let ready = harness.drive_ready(&admitted.id).await;
        let unrelated_id = SandboxId::generate();
        let unrelated = sandbox(&unrelated_id, ready.owner.clone(), &rfc3339_at(harness.now));
        let unrelated_runtime = harness
            .adapter
            .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
                1,
                sandbox_spec(&unrelated),
            ))
            .await
            .unwrap();
        harness
            .adapter
            .run_install(
                &unrelated_runtime,
                &InstallRequest::new("unrelated-install", Vec::new()),
            )
            .await
            .unwrap();
        harness
            .adapter
            .deliver_credential(
                &unrelated_runtime,
                CredentialFrame {
                    operation_id: "unrelated-credential".into(),
                    value: RuntimeCredential::new("unrelated-fixture-authority"),
                    custody_generation: Some(1),
                    github_route: None,
                },
            )
            .await
            .unwrap();
        let unrelated_before = harness
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .into_iter()
            .find(|value| value.sandbox_id == unrelated_id.as_str())
            .unwrap();
        let app = router(&harness);
        assert_eq!(
            call(&app, &ready, Method::DELETE, "").await,
            StatusCode::CONFLICT
        );
        retire_paused_parent_with_pending_stop(&harness, &ready, state != "present", None).await;
        assert_eq!(
            call(&app, &ready, Method::POST, "/stop").await,
            StatusCode::ACCEPTED
        );
        harness
            .backend
            .set_inventory_unavailable(state == "unavailable");
        for _ in 0..3 {
            harness
                .worker
                .run_one(&ready.id, SystemTime::now())
                .await
                .unwrap();
        }
        let stopped = harness.stored(&ready.id).await;
        // Even the unavailable-inventory case must leave the unrelated runtime
        // intact; restore only the fixture probe to inspect the same identity.
        harness.backend.set_inventory_unavailable(false);
        assert_eq!(
            harness
                .adapter
                .runtime_inventory()
                .await
                .unwrap()
                .into_iter()
                .find(|value| value.sandbox_id == unrelated_id.as_str()),
            Some(unrelated_before.clone())
        );
        if state != "absent" {
            assert_eq!(stopped.status, AgentStatus::Paused);
            assert_eq!(stopped.work.step, AgentWorkStep::CloseProcess);
            assert!(
                stopped.work.attempts > 0,
                "{state} must record the refused cleanup attempt"
            );
            assert_eq!(
                call(&app, &ready, Method::DELETE, "").await,
                StatusCode::CONFLICT
            );
            continue;
        }
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert_eq!(stopped.stop_reason, Some(AgentStopReason::Owner));
        assert_eq!(
            stopped.conversation.conversation_id,
            ready.conversation.conversation_id
        );
        assert_eq!(stopped.work.receipts, ready.work.receipts);
        // The provider is independently absent; finish the parent fixture's
        // remaining finality steps before exercising record-only deletion.
        let mut parent = harness
            .store
            .get_sandbox(&ready.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        parent.credential.current = None;
        parent.credential.delivered = false;
        let at = rfc3339_at(SystemTime::now());
        parent
            .transition(SandboxStatus::Retired, at.clone(), None)
            .unwrap();
        parent.work.schedule(SandboxWorkStep::WaitForPurge, at);
        harness.store.save_sandbox(revision, parent).await.unwrap();
        for _ in 0..2 {
            assert_eq!(
                call(&app, &ready, Method::DELETE, "").await,
                StatusCode::ACCEPTED
            );
        }
        let effects = harness.backend.requests().await;
        for _ in 0..4 {
            harness
                .worker
                .run_one(&ready.id, SystemTime::now())
                .await
                .unwrap();
        }
        assert_eq!(harness.stored(&ready.id).await.status, AgentStatus::Deleted);
        assert_eq!(
            harness.backend.requests().await,
            effects,
            "cleanup cannot restart work or invent runtime receipts"
        );
        assert_eq!(
            harness
                .adapter
                .runtime_inventory()
                .await
                .unwrap()
                .into_iter()
                .find(|value| value.sandbox_id == unrelated_id.as_str()),
            Some(unrelated_before)
        );
    }
}
