mod server;
mod writes;
use super::*;
use server::Server;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pooled_http1_and_multiplexed_http2_observe_writes_before_provider_processing() {
    for h2 in [false, true] {
        for tls in [false, true] {
            let server = Server::start(h2, tls, Duration::from_millis(120)).await;
            let transport = server.client(h2);
            // Sequential calls prove actual connection reuse for both protocols and TLS.
            for index in 0..3 {
                let timing = IngressTiming::new("custom");
                let received = timing.received();
                let observed = transport
                    .send(
                        transport.origin.join("responses").unwrap(),
                        "runtime-provider",
                        &index.to_string(),
                        b"{}".to_vec(),
                        Some(timing.clone()),
                    )
                    .await
                    .unwrap();
                assert!(received.elapsed() >= Duration::from_millis(120));
                assert!(observed.written_at.duration_since(received) < Duration::from_millis(100));
                assert!(!timing.header().contains("not_written"));
                assert_eq!(observed.response.status(), 200);
                observed.response.bytes().await.unwrap();
            }
            assert_eq!(
                server.connections.load(Ordering::SeqCst),
                1,
                "h2={h2} tls={tls}"
            );
            if h2 {
                let requests = (0..64).map(|index| {
                    let transport = &transport;
                    async move {
                        let timing = IngressTiming::new("custom");
                        let received = timing.received();
                        let response = transport
                            .send(
                                transport.origin.join("responses").unwrap(),
                                "runtime-provider",
                                &format!("concurrent-{index}"),
                                vec![b'x'; 80_000 + index],
                                Some(timing.clone()),
                            )
                            .await
                            .unwrap();
                        assert!(response.written_at >= received);
                        assert!(!timing.header().contains("not_written"));
                        response.response.bytes().await.unwrap();
                    }
                });
                tokio::time::timeout(
                    Duration::from_secs(5),
                    futures_util::future::join_all(requests),
                )
                .await
                .unwrap();
                assert_eq!(server.connections.load(Ordering::SeqCst), 1);
                assert_eq!(server.requests.lock().unwrap().len(), 67);
                assert!(transport
                    .multiplexed
                    .lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .pending
                    .lock()
                    .unwrap()
                    .is_empty());
            }
        }
    }
}

#[tokio::test]
async fn tls_verifies_certificates_and_h1_terminal_tail_reuses_the_connection() {
    let server = Server::start(false, true, Duration::ZERO).await;
    let mut transport = server.client(false);
    let trusted = transport.tls.clone();
    transport.tls = connect::tls_config();
    assert!(transport
        .send(
            transport.origin.join("responses").unwrap(),
            "runtime-provider",
            "bad-tls",
            b"{}".to_vec(),
            None
        )
        .await
        .is_err());
    transport.tls = trusted;
    let mut response = transport
        .send(
            transport.origin.join("responses").unwrap(),
            "runtime-provider",
            "terminal",
            b"{}".to_vec(),
            None,
        )
        .await
        .unwrap();
    response.response.chunk().await.unwrap().unwrap();
    response.complete.complete();
    drop(response);
    tokio::time::timeout(Duration::from_secs(1), async {
        while transport.idle.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = transport
        .send(
            transport.origin.join("responses").unwrap(),
            "runtime-provider",
            "reuse",
            b"{}".to_vec(),
            None,
        )
        .await
        .unwrap();
    response.response.bytes().await.unwrap();
    assert_eq!(server.connections.load(Ordering::SeqCst), 2); // One untrusted handshake, one reused connection.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_clears_http2_observations_and_preserves_other_streams() {
    let (server, responses) = Server::start_gated().await;
    let transport = Arc::new(server.client(true));
    let mut cancelled = tokio::task::JoinSet::new();
    for index in 0..64 {
        let transport = transport.clone();
        cancelled.spawn(async move {
            let id = format!("cancelled-{index}");
            transport
                .send(
                    transport.origin.join("responses").unwrap(),
                    "runtime-provider",
                    &id,
                    vec![b'x'; 50_000],
                    None,
                )
                .await
        });
    }
    let survivor = tokio::spawn({
        let transport = transport.clone();
        async move {
            transport
                .send(
                    transport.origin.join("responses").unwrap(),
                    "runtime-provider",
                    "survivor",
                    b"{}".to_vec(),
                    None,
                )
                .await
        }
    });
    // Cancel accepted streams, with one live stream sharing their connection.
    // A timer can instead cancel before acceptance and trip h2's reset-flood limit.
    server.wait_for_requests(65).await;
    let pending = transport
        .multiplexed
        .lock()
        .await
        .as_ref()
        .unwrap()
        .pending
        .clone();
    cancelled.abort_all();
    while let Some(result) = cancelled.join_next().await {
        assert!(matches!(result, Err(error) if error.is_cancelled()));
    }
    assert!(pending.lock().unwrap().is_empty());
    assert!(!survivor.is_finished());
    responses.add_permits(65);
    let response = tokio::time::timeout(Duration::from_secs(5), survivor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    response.response.bytes().await.unwrap();
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
    assert!(pending.lock().unwrap().is_empty());
}
