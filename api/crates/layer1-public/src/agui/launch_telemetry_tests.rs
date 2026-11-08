use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, Kind};
#[tokio::test(start_paused = true)]
async fn launch_refresh_counts_quiet_output_and_each_failure() {
    for failure in [
        LaterFailure::Missing,
        LaterFailure::OwnershipLost,
        LaterFailure::LaunchRead,
        LaterFailure::EventRead,
    ] {
        isolated_test(async {
            let store = TestLaunchStore::new([]);
            let response = response(
                &store,
                request("/api/v1/agui/launches/lch_one/events", OWNER),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(snapshot(Kind::LaunchEvents), (1, [0; 3]));
            let mut body = response.into_body().into_data_stream();
            assert!(
                tokio::time::timeout(Duration::from_millis(800), body.next())
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(Kind::LaunchEvents), (1, [3, 0, 0]));
            let event = store.append(LaunchEventKind::Created);
            let frame = body.next().await.unwrap().unwrap();
            assert_eq!(
                frames(&frame)[0].1,
                serde_json::to_string(&LaunchEventView::from(event)).unwrap()
            );
            assert_eq!(snapshot(Kind::LaunchEvents), (1, [3, 1, 0]));
            match failure {
                LaterFailure::Missing => store.hide(),
                LaterFailure::OwnershipLost => store.transfer(),
                LaterFailure::LaunchRead => store.fail_launches(),
                LaterFailure::EventRead => store.fail_event_reads(),
            }
            assert!(body.next().await.is_none());
            assert_eq!(snapshot(Kind::LaunchEvents), (0, [3, 1, 1]));
            drop(body);
            assert_eq!(snapshot(Kind::LaunchEvents).0, 0);
        })
        .await;
    }
}
