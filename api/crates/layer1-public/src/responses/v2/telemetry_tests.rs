use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, Kind};

#[tokio::test(start_paused = true)]
async fn replayed_response_frames_are_quiet_until_new_durable_output_arrives() {
    isolated_test(async {
        let f = fixture().await;
        let response = create(&f, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(snapshot(Kind::ResponsesV2).0, 1);
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
        let (_, quiet) = snapshot(Kind::ResponsesV2);
        assert!(quiet[0] >= 3);
        assert_eq!(quiet[1..], [0, 0]);
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
        assert_eq!(snapshot(Kind::ResponsesV2).1[1], 1);
        while body.next().await.is_some() {}
        assert_eq!(snapshot(Kind::ResponsesV2).0, 0);
    })
    .await;
}
