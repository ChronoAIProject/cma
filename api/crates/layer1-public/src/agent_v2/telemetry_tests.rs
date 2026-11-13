use super::*;
use crate::{
    agent_v2::fixtures::*,
    stream_telemetry::{isolated_test, snapshot as counts},
};
use axum::http::{Method, StatusCode};
use futures_util::StreamExt;
use layer2_core::sandbox::AgentLogKind;
use tower::ServiceExt;

#[tokio::test(start_paused = true)]
async fn public_tail_records_quiet_and_output_polls_and_releases_on_disconnect() {
    isolated_test(async {
        let f = fixture(true, true).await;
        let response = f
            .router
            .clone()
            .oneshot(request(Method::GET, EVENTS, "owner-a", None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(counts(Kind::AgentEventsV2).0, 1);
        let mut body = response.into_body().into_data_stream();
        // Drain initial replay and let several quiet polls run.
        while tokio::time::timeout(Duration::from_millis(350), body.next())
            .await
            .is_ok()
        {}
        let (_, before) = counts(Kind::AgentEventsV2);
        assert!(before[0] >= 3);
        assert_eq!(before[2], 0);
        append(
            &f.store,
            AgentLogKind::OwnerMessage {
                content: "new output".into(),
            },
        )
        .await;
        let frame = body.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&frame).contains("new output"));
        let (_, after) = counts(Kind::AgentEventsV2);
        assert_eq!(after[1], before[1] + 1);
        drop(body);
        assert_eq!(counts(Kind::AgentEventsV2).0, 0);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn failed_open_never_counts_as_active() {
    isolated_test(async {
        let f = fixture(true, true).await;
        let denied = f
            .router
            .clone()
            .oneshot(request(Method::GET, EVENTS, "stranger", None, None))
            .await
            .unwrap();
        assert_ne!(denied.status(), StatusCode::OK);
        assert_eq!(counts(Kind::AgentEventsV2), (0, [0; 3]));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn failed_tail_refresh_is_not_quiet() {
    isolated_test(async {
        let f = fixture(true, true).await;
        let auth = AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: None,
        };
        let mut open = open(
            f.state,
            auth,
            AgentId("agt_fixture".into()),
            None,
            HeaderMap::new(),
        )
        .await
        .unwrap();
        open.pending.clear();
        open.initial_gap = false;
        // An invalid incarnation is detected during the next refresh.
        open.identity = "changed".into();
        let mut body = response(open).into_body().into_data_stream();
        let error = body.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&error).contains("cma.agent.error"));
        assert_eq!(counts(Kind::AgentEventsV2).1, [0, 0, 1]);
        assert!(body.next().await.is_none());
        assert_eq!(counts(Kind::AgentEventsV2).0, 0);
    })
    .await;
}
