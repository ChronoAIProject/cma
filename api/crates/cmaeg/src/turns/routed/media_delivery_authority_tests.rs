use super::super::{model, storage, Authority, ExecutionError};
use super::*;

#[tokio::test]
async fn media_finish_rechecks_authority_source_sink_and_expiry_before_restoring_terminal_payload()
{
    use std::sync::atomic::Ordering;
    struct Unavailable;
    impl Authority for Unavailable {
        fn current<'a>(
            &'a self,
            _: &'a crate::ingress::routing_store::EventIdentity,
        ) -> futures_util::future::BoxFuture<
            'a,
            Result<Arc<crate::channels::resolver::ResolvedChannel>, ExecutionError>,
        > {
            Box::pin(async { Err(ExecutionError::Unavailable) })
        }
    }
    for scenario in 0..5 {
        let mut f = Fixture::new().await;
        let event = model::delivery_event(&f.setup.plan, f.setup.target());
        f.custom.local_response(
            &event,
            &CustomStreamEffect::Completed {
                final_text: Some("private original result".into()),
            },
            "resp_1",
        );
        f.store
            .refuse(&mut f.lease, "authority_revoked")
            .await
            .unwrap();
        f.store.release(&mut f.lease).await.unwrap();
        match scenario {
            0 => f.setup.current.revoke_at.store(0, Ordering::SeqCst),
            1 => Arc::make_mut(&mut *f.setup.current.channel.lock().unwrap())
                .definition
                .sources
                .clear(),
            2 => Arc::make_mut(&mut *f.setup.current.channel.lock().unwrap())
                .definition
                .reply
                .sinks
                .clear(),
            3 => f
                .setup
                .server
                .memory
                .clock
                .store(f.setup.target().launch.event.expires_at, Ordering::SeqCst),
            _ => f.delivery.authority = Arc::new(Unavailable),
        }
        f.delivery
            .finish(&f.setup.plan, f.setup.target())
            .await
            .unwrap();
        let stream = f.stream().await;
        let final_frame = stream.split("event: cmaeg.failed\n").nth(1).unwrap();
        assert!(
            !final_frame.contains("private original result"),
            "{scenario}: {stream}"
        );
        let data: serde_json::Value = serde_json::from_str(
            final_frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(data["text"], serde_json::Value::Null);
        assert_eq!(data["message"], serde_json::Value::Null);
        let (status, reason) = match scenario {
            3 => (410, "media_stream_event_expired"),
            4 => (503, "media_stream_authority_unavailable"),
            _ => (403, "media_stream_authority_revoked"),
        };
        assert_eq!(data["error"]["detail"], reason);
        assert_eq!(data["error"]["status"], status);
        assert_eq!(data["media_complete"], false);
        let id = storage::identifier(&("media_stream_close_v1", &f.setup.target().id));
        let receipt = f
            .store
            .0
            .read::<serde_json::Value>(&id)
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(receipt["content_suppressed"], true);
    }
}
