use super::*;

use axum::{body::Body, routing::post, Router};
use cma_am_auth::request_audit::Credential;
use layer2_core::{ActivityId, ActivityQuery, ActivityStore, Page, SubjectId};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use tower::ServiceExt;

#[derive(Default)]
pub(in crate::activity) struct Store {
    pub(in crate::activity) rows: Mutex<Vec<ActivityRecord>>,
    reserve_failure: AtomicBool,
    finish_failure: AtomicBool,
}

#[async_trait::async_trait]
impl ActivityStore for Store {
    async fn append(&self, row: &ActivityRecord) -> Result<(), StoreError> {
        if self.reserve_failure.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("fixture".into()));
        }
        self.rows.lock().unwrap().push(row.clone());
        Ok(())
    }

    async fn attempt(&self, id: &ActivityId) -> Result<Option<ActivityRecord>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|row| &row.id == id)
            .cloned())
    }

    async fn identify_attempt(&self, row: &ActivityRecord) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let pending = rows
            .iter_mut()
            .find(|pending| pending.id == row.id)
            .unwrap();
        assert!(pending.actor.is_none());
        *pending = row.clone();
        Ok(())
    }

    async fn finish_attempt(
        &self,
        id: &ActivityId,
        outcome: &ActivityOutcome,
        status: u16,
    ) -> Result<(), StoreError> {
        if self.finish_failure.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("fixture".into()));
        }
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|row| &row.id == id).unwrap();
        assert_eq!(
            row.outcome,
            ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into())
        );
        row.outcome = outcome.clone();
        row.detail.record(DetailKey::HttpStatus, status.to_string());
        Ok(())
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(self.rows.lock().unwrap().clone(), None))
    }
}

#[tokio::test]
async fn audit_binds_verified_identity_and_final_result_to_one_attempt() {
    let store = Arc::new(Store::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/v1/agents/{id}/input",
            post({
                let calls = calls.clone();
                let store = store.clone();
                move |headers: axum::http::HeaderMap| {
                    let calls = calls.clone();
                    let store = store.clone();
                    async move {
                        let (channel, credential) = if headers.contains_key("fixture-session") {
                            (ActivityChannel::Web, Credential::WebSession)
                        } else {
                            (ActivityChannel::Api, Credential::AgentKey)
                        };
                        request_audit::bind(VerifiedActor {
                            subject: SubjectId::new("verified"),
                            channel,
                            credential,
                            scope: "personal",
                        })
                        .await
                        .unwrap();
                        assert!(store.rows.lock().unwrap().iter().any(|row| {
                            row.actor.as_ref() == Some(&SubjectId::new("verified"))
                                && row.outcome
                                    == ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into())
                        }));
                        calls.fetch_add(1, Ordering::SeqCst);
                        StatusCode::ACCEPTED
                    }
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            Admission::new(
                &ActivityRecorder::new(ActivityService::new(store.clone())),
                None,
            ),
            audit,
        ));
    let submitted_id = "00000000-0000-0000-0000-000000000001";
    let target = "agt_0123456789abcdef0123456789abcdef";
    let send = |session: bool| {
        let mut request =
            Request::post(format!("/api/v1/agents/{target}/input?credential=discard"))
                .header("x-request-id", submitted_id)
                .header("x-cma-client", "sdk")
                .header("x-cma-actor", "forged")
                .body(Body::from("prompt-secret"))
                .unwrap();
        if session {
            request
                .headers_mut()
                .insert("fixture-session", HeaderValue::from_static("true"));
        }
        request
    };
    for (index, channel) in ["web", "sdk"].into_iter().enumerate() {
        let response = app.clone().oneshot(send(index == 0)).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let returned = response.headers()["x-request-id"].to_str().unwrap();
        assert_ne!(returned, submitted_id);
        let rows = store.rows.lock().unwrap();
        let row = &rows[index];
        assert_eq!(row.detail.get(DetailKey::RequestId), Some(returned));
        assert_eq!(row.detail.get(DetailKey::ActivityChannel), Some(channel));
        assert_eq!(
            row.detail.get(DetailKey::RequestRoute),
            Some("/api/v1/agents/{id}/input")
        );
        assert_eq!(row.detail.get(DetailKey::HttpStatus), Some("202"));
        assert_eq!(row.target, Some(ActivityTarget::Resource(target.into())));
        let serialized = serde_json::to_string(row).unwrap();
        for forbidden in [
            "prompt-secret",
            "credential=discard",
            "forged",
            submitted_id,
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
    store.reserve_failure.store(true, Ordering::SeqCst);
    assert_eq!(
        app.clone().oneshot(send(false)).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    store.reserve_failure.store(false, Ordering::SeqCst);
    store.finish_failure.store(true, Ordering::SeqCst);
    assert_eq!(
        app.oneshot(send(false)).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let rows = store.rows.lock().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[2].outcome,
        ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into())
    );
    assert_ne!(
        rows[0].detail.get(DetailKey::RequestId),
        rows[1].detail.get(DetailKey::RequestId)
    );
}
