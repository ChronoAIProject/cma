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
    failure: Option<AgentHistoryExportError>,
}

#[async_trait::async_trait]
impl AgentHistoryExporter for Exporter {
    async fn export(&self, _: &str, _: &str) -> Result<Vec<u8>, AgentHistoryExportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.blocked {
            self.resume.notified().await;
        }
        match self.failure {
            Some(error) => Err(error),
            None => Ok(NATIVE.to_vec()),
        }
    }
}

#[tokio::test]
async fn raw_history_failures_have_safe_codes_without_native_bytes() {
    for (failure, code) in [
        (
            AgentHistoryExportError::Unavailable,
            "history_export_unavailable",
        ),
        (
            AgentHistoryExportError::Unsupported,
            "history_export_unsupported",
        ),
        (AgentHistoryExportError::Rejected, "history_export_rejected"),
    ] {
        let (fixture, _, _, _) = fixture(Reply::Member).await;
        let exporter = Arc::new(Exporter {
            failure: Some(failure),
            ..Default::default()
        });
        let router = agent_router(fixture.state.with_history_exporter(exporter));
        let response = router.oneshot(request("owner-a", true)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], code);
        assert!(!String::from_utf8_lossy(&body).contains("private-native-sentinel"));
    }
}

#[tokio::test]
async fn retained_transcript_survives_stopped_failed_and_paused_without_runtime_io() {
    use layer2_core::sandbox::{AgentStatus, SandboxStatus};
    for status in [
        AgentStatus::Ready,
        AgentStatus::Stopped,
        AgentStatus::Failed,
    ] {
        let (fixture, _, _, _) = fixture(Reply::Missing).await;
        let mut agent = fixture
            .store
            .get_agent(&AgentId("agt_shared_read".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.status = status;
        agent.revision += 1;
        fixture
            .store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
        let mut sandbox = fixture
            .store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox.status = SandboxStatus::Paused;
        sandbox.runtime = None;
        sandbox.revision += 1;
        fixture.store.save_sandbox(revision, sandbox).await.unwrap();
        let exporter = Arc::new(Exporter {
            failure: Some(AgentHistoryExportError::Unavailable),
            ..Default::default()
        });
        let router = agent_router(fixture.state.with_history_exporter(exporter.clone()));
        for verified in [false, true] {
            let response = router
                .clone()
                .oneshot(request("owner-a", verified))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(problem["code"], "history_export_unavailable");
        }
        for subject in ["owner-a", "foreign"] {
            let mut read = request(subject, true);
            *read.uri_mut() = "/api/v1/agents/agt_shared_read/transcript?limit=32"
                .parse()
                .unwrap();
            let response = router.clone().oneshot(read).await.unwrap();
            if subject == "owner-a" {
                assert_eq!(response.status(), StatusCode::OK, "{status:?}");
                let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                let data: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(data["agent_id"], "agt_shared_read");
                assert!(!data["turns"].as_array().unwrap().is_empty());
            } else {
                assert!(response.status().is_client_error());
            }
        }
        assert_eq!(exporter.calls.load(Ordering::SeqCst), 0);
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
