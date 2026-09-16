use super::*;
use crate::bridge::ingress::event_cases;

#[tokio::test]
async fn signed_unsupported_event_http_matrix_retains_only_denied_receipts() {
    for platform in event_cases::platforms() {
        for case in event_cases::cases(&platform) {
            let mut fixture = HttpFixture::for_platform(false, platform.clone());
            event_cases::text(
                &mut fixture.protocol.body,
                &platform,
                "CMA-CONTROL/1 cancel ccr_source",
            );
            case.apply(&mut fixture.protocol.body, &platform);
            fixture.provider.commits.add_permits(2);
            let response = fixture.router().oneshot(fixture.request()).await.unwrap();
            let Some(reason) = case.expected(&platform) else {
                assert_eq!(
                    response.status(),
                    StatusCode::BAD_REQUEST,
                    "{platform:?} {case:?}"
                );
                assert_eq!(value(response).await["code"], "callback_payload_invalid");
                assert_eq!(fixture.provider.inbox_calls.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.provider.provider_calls.load(Ordering::SeqCst), 0);
                assert!(fixture.provider.rows.lock().await.is_empty());
                continue;
            };
            assert_eq!(
                response.status(),
                StatusCode::ACCEPTED,
                "{platform:?} {case:?}"
            );
            let original = value(response).await;
            assert_eq!(original["status"], "inbox_persisted");
            assert_eq!(
                original["receipt"]["admission"],
                json!({"partition_denied":{"reason":reason}})
            );
            assert!(original["receipt"].get("execution_hold").is_none());
            let replay = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
            assert_eq!(replay, original);
            assert!(fixture.provider.refusals.lock().await.is_empty());
            let rows = fixture.provider.rows.lock().await;
            assert_eq!(rows.len(), 1);
            assert!(rows
                .values()
                .all(|row| row.admission != InboxAdmission::Accepted));
        }
    }
}
