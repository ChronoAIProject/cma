use super::{
    agents::agent_router,
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
    AgentHistoryExportError, AgentHistoryExporter,
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::store::{AgentStore, SandboxStore},
    AgentId,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;
use tower::ServiceExt;

const PATH: &str = "/api/v1/agents/agt_shared_read/history";
const NATIVE: &[u8] = b"{\"private-native-sentinel\":true}\n";

#[derive(Default)]
struct Exporter {
    entered: Notify,
    resume: Notify,
    calls: AtomicUsize,
    blocked: bool,
}

#[async_trait::async_trait]
impl AgentHistoryExporter for Exporter {
    async fn export(&self, _: &str, _: &str) -> Result<Vec<u8>, AgentHistoryExportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.blocked {
            self.resume.notified().await;
        }
        Ok(NATIVE.to_vec())
    }
}

fn request(subject: &str, verified: bool) -> Request<Body> {
    let mut request = agents_tests::request(Method::GET, PATH, subject, None, None);
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
    }
    request
}

#[tokio::test]
async fn raw_history_allows_owner_and_denies_shared_and_foreign_before_export() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let exporter = Arc::new(Exporter::default());
    let router = agent_router(fixture.state.with_history_exporter(exporter.clone()));
    for verified in [false, true] {
        for subject in ["recipient", "foreign"] {
            let response = router
                .clone()
                .oneshot(request(subject, verified))
                .await
                .unwrap();
            assert!(response.status().is_client_error());
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            assert!(!String::from_utf8_lossy(&body).contains("private-native-sentinel"));
        }
        let response = router
            .clone()
            .oneshot(request("owner-a", verified))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            &to_bytes(response.into_body(), 8192).await.unwrap()[..],
            NATIVE
        );
    }
    assert_eq!(exporter.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn thousand_revoke_export_interleavings_release_no_native_bytes() {
    for n in 0..1000 {
        let (fixture, access, _, share) = fixture(Reply::Member).await;
        let exporter = Arc::new(Exporter {
            blocked: true,
            ..Default::default()
        });
        let router = agent_router(fixture.state.with_history_exporter(exporter.clone()));
        let pending = tokio::spawn(async move {
            router
                .oneshot(request("owner-a", n % 2 == 0))
                .await
                .unwrap()
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            exporter.entered.notified(),
        )
        .await
        .unwrap();
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector.clone(),
                    change: ShareChange::Revoke {
                        share_id: share.id.clone(),
                    },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "revoke-during-export".into(),
                },
            )
            .await
            .unwrap();
        exporter.resume.notify_one();
        let response = pending.await.unwrap();
        assert!(response.status().is_client_error(), "interleaving {n}");
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private-native-sentinel"));
        assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn runtime_and_native_conversation_changes_during_export_discard_bytes() {
    for change in [
        "runtime",
        "binding",
        "conversation",
        "generation",
        "deleted",
    ] {
        let (fixture, _, _, _) = fixture(Reply::Member).await;
        let exporter = Arc::new(Exporter {
            blocked: true,
            ..Default::default()
        });
        let router = agent_router(fixture.state.with_history_exporter(exporter.clone()));
        let pending =
            tokio::spawn(async move { router.oneshot(request("owner-a", true)).await.unwrap() });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            exporter.entered.notified(),
        )
        .await
        .unwrap();
        let mut agent = fixture
            .store
            .get_agent(&AgentId("agt_shared_read".into()))
            .await
            .unwrap()
            .unwrap();
        if matches!(change, "runtime" | "binding") {
            let mut sandbox = fixture
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            let previous = sandbox.revision;
            let runtime = sandbox.runtime.as_mut().unwrap();
            if change == "runtime" {
                runtime.runtime_id = "replacement-runtime".into();
            } else {
                runtime.bound_at = "2026-11-10T00:00:00Z".into();
            }
            sandbox.revision += 1;
            fixture.store.save_sandbox(previous, sandbox).await.unwrap();
        } else {
            let previous = agent.revision;
            match change {
                "conversation" => {
                    agent.conversation.conversation_id = Some("replacement-conversation".into())
                }
                "generation" => agent.conversation.runtime_generation = Some(999),
                _ => agent.deleted_at = Some("2026-11-10T00:00:00Z".into()),
            }
            agent.revision += 1;
            fixture.store.save_agent(previous, agent).await.unwrap();
        }
        exporter.resume.notify_one();
        let response = pending.await.unwrap();
        assert!(response.status().is_client_error(), "{change}");
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private-native-sentinel"));
    }
}
