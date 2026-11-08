use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, Kind};

#[tokio::test(start_paused = true)]
async fn replayed_response_frames_are_quiet_until_new_durable_output_arrives() {
    isolated_test(async {
        let f = fixture().await;
        let response = create(&f, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(800), body.next())
                .await
                .is_err()
        );
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [3, 0, 0]));
        let record = f
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0);
        f.store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let output = events(&body.next().await.unwrap().unwrap());
        assert_eq!(output[0]["type"], "response.failed");
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [3, 1, 0]));
        while body.next().await.is_some() {}
        assert_eq!(snapshot(Kind::ResponsesV2), (0, [3, 1, 0]));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn expired_response_authorization_counts_failed_refresh_and_releases_stream() {
    crate::stream_telemetry::isolated_test(async {
        let f = fixture().await;
        let response = create(&f, false).await;
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        drop(response);
        assert_eq!(snapshot(Kind::ResponsesV2), (0, [0; 3]));
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: AgentId("agt_fixture".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "share".into(),
                },
            )
            .await
            .unwrap();
        let response = f
            .router
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/api/v2/responses/{id}/events"),
                "reader",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0; 3]));
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            events(&body.next().await.unwrap().unwrap())[0]["type"],
            "response.created"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        let event = events(&body.next().await.unwrap().unwrap());
        assert_eq!(event[0]["type"], "error");
        assert_eq!(snapshot(Kind::ResponsesV2), (1, [0, 0, 1]));
        assert!(body.next().await.is_none());
        assert_eq!(
            f.store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items[0]
                .status,
            AgentResponseStatus::Queued
        );

        assert_eq!(
            crate::stream_telemetry::snapshot(crate::stream_telemetry::Kind::ResponsesV2),
            (0, [0, 0, 1])
        );
    })
    .await;
}
