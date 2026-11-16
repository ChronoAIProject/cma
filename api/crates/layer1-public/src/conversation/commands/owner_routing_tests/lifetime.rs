use super::*;
use axum::http::StatusCode;
use futures_util::StreamExt;
use layer2_core::sandbox::{store::AgentLogStore, store::AgentStore, AgentLogKind};
use tokio::sync::{Notify, Semaphore};
use tower::ServiceExt;

struct PausedAdmission {
    inner: Arc<Router>,
    entered: Notify,
    release: Semaphore,
    finished: Notify,
}

#[async_trait]
impl AgentCommandOwnerRouting for PausedAdmission {
    async fn route(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        let result = self
            .inner
            .route(context, credential, source, mutation)
            .await;
        self.finished.notify_one();
        result
    }
}

#[tokio::test]
async fn dropped_legacy_http_request_keeps_owner_admission_and_same_key_receipt() {
    for path in ["/api/v1/agui/run", "/api/v1/responses"] {
        let mut f = fixture(None).await;
        let paused = Arc::new(PausedAdmission {
            inner: f.router.clone(),
            entered: Notify::new(),
            release: Semaphore::new(0),
            finished: Notify::new(),
        });
        let authority = Arc::new(Authority);
        let access = Arc::new(ResourceAccessService::new(
            f.store.clone(),
            authority.clone(),
        ));
        f.state.commands = Some(
            AgentCommandService::new(f.ingress.clone(), access, authority)
                .with_owner_routing(paused.clone()),
        );
        let evidence = OwnerCommandRoutingEvidence::new(Arc::from(&b"dropped-origin"[..]));
        let task_evidence = evidence.clone();
        let request = legacy::routes(&f).oneshot(legacy::request(path));
        let pending = tokio::spawn(async move { task_evidence.scope(request).await });
        tokio::time::timeout(Duration::from_secs(2), paused.entered.notified())
            .await
            .unwrap();
        assert!(records(&f).await.is_empty());
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        // Release only after the HTTP future is gone. The detached admission
        // task must retain original evidence and acquire its own owner scope.
        paused.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), paused.finished.notified())
            .await
            .unwrap();
        let committed = records(&f).await;
        assert_eq!(committed.len(), 1, "{path}");
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            *f.router.evidence.lock().unwrap(),
            vec![Some(b"dropped-origin".to_vec())]
        );
        let mutation = f.router.mutations.lock().unwrap()[0].clone();
        let OwnerCommandMutation::Admit { submission, .. } = mutation else {
            panic!("legacy route must submit the original command");
        };
        let replay = evidence.scope(admit(&f, submission)).await.unwrap();
        assert_eq!(
            replay.receipt.command_id,
            committed[0].receipt.envelope.command_id
        );
        assert_eq!(replay.receipt.response_id, committed[0].receipt.response_id);
        assert_eq!(records(&f).await.len(), 1);
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn ingress_stream_outlives_finite_owner_admission_and_relay_timeouts() {
    for path in ["/api/v1/agui/run", "/api/v1/responses", "/api/v2/responses"] {
        let f = fixture(None).await;
        let router = command_test_support::router(f.state.clone(), f.store.clone());
        let evidence = OwnerCommandRoutingEvidence::new(Arc::from(&b"long-stream-origin"[..]));
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            evidence.scope(router.oneshot(legacy::request(path))),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()["x-cma-command-id"],
            records(&f).await[0].receipt.envelope.command_id.as_str()
        );
        let mut stream = response.into_body().into_data_stream();
        assert!(stream.next().await.unwrap().is_ok());
        // The finite mutation has completed. Observer lifetime must not borrow
        // the mutation's owner token or the peer's 10s read / 60s total budget.
        f.router.authority.revoke();
        let began = tokio::time::Instant::now();
        tokio::time::advance(Duration::from_secs(65)).await;
        let mut agent = f
            .store
            .get_agent(&AgentId(ID.into()))
            .await
            .unwrap()
            .unwrap();
        f.store
            .append_agent_log(
                &agent.id,
                layer2_core::now_rfc3339(),
                AgentLogKind::AgentMessage {
                    content: "output-after-sixty-five-seconds".into(),
                },
            )
            .await
            .unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.conversation.last_seq += 1;
        f.store.save_agent(revision, agent).await.unwrap();
        let mut observed = false;
        for _ in 0..20 {
            let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("a live ingress observer must deliver the late output")
                .expect("stream must not inherit the finite owner relay deadline")
                .unwrap();
            if std::str::from_utf8(&frame)
                .unwrap()
                .contains("output-after-sixty-five-seconds")
            {
                observed = true;
                break;
            }
        }
        assert!(observed, "{path}");
        assert!(began.elapsed() >= Duration::from_secs(65));
        assert_eq!(f.router.mutations.lock().unwrap().len(), 1);
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
        assert_eq!(records(&f).await.len(), 1);
        drop(stream);
    }
}
