//! Compatibility admission executes the original identity under current authority.
use super::{test_support::*, *};
use crate::ingress::routing_store::{test_server::Server, Candidate};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[tokio::test]
async fn compatibility_dispatch_preserves_request_and_refuses_revoked_current_authority() {
    for revoke in [false, true] {
        let request = fixture::request(
            |v| {
                v["routing"] = serde_json::Value::Null;
                v["secrets"]["consumer_keys"] = serde_json::json!([]);
            },
            fixture::message("original compatibility message"),
        );
        let server = Server::start().await;
        let candidate =
            Candidate::prepare_compatibility(&request, b"wire", &fixture::keys(), fixture::NOW)
                .unwrap();
        let plan = server.store().admit(&candidate).await.unwrap();
        let mut journal = Journal::new(server.records.clone(), Arc::new(fixture::keys()));
        let clock = server.memory.clock.clone();
        journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
        let current = Arc::new(Current {
            channel: Mutex::new(request.channel.clone()),
            calls: AtomicUsize::new(0),
            revoke_at: AtomicUsize::new(if revoke { 2 } else { usize::MAX }),
        });
        let port = Arc::new(Port {
            starts: Mutex::new(Vec::new()),
            prepares: AtomicUsize::new(0),
            support: supported(),
            next: Mutex::new(None),
            input_size: 32,
        });
        let dispatcher = Dispatcher::new(server.store(), journal.clone(), current, port.clone());
        let result = dispatcher
            .dispatch(&plan.identity, &plan.targets[0].id, "compatibility-fixture")
            .await;
        if revoke {
            assert!(matches!(result, Err(ExecutionError::AuthorityChanged)));
            assert!(port.starts.lock().unwrap().is_empty());
        } else {
            let DispatchOutcome::Replay {
                mut lease, body, ..
            } = result.unwrap()
            else {
                panic!("provider accepted")
            };
            assert_eq!(body.conversation_id, request.conversation_id);
            assert_eq!(body.idempotency_key, request.idempotency_key);
            assert_eq!(body.event_context.activity, request.activity);
            assert_eq!(
                body.event_context
                    .event_ref
                    .open(&fixture::keys(), fixture::NOW)
                    .unwrap()
                    .payload,
                request
                    .event_ref
                    .open(&fixture::keys(), fixture::NOW)
                    .unwrap()
                    .payload
            );
            journal.release(&mut lease).await.unwrap();
            assert!(matches!(
                dispatcher
                    .dispatch(&plan.identity, &plan.targets[0].id, "restored")
                    .await
                    .unwrap(),
                DispatchOutcome::Replay { .. }
            ));
            assert_eq!(
                port.starts.lock().unwrap().len(),
                1,
                "accepted provider work is replayed, not resubmitted"
            );
        }
    }
}
