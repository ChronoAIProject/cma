use super::*;

// Exercise real transport negotiation and exact-token receiver revocation.
pub(super) struct Receiver {
    _transport: Arc<layer3_adapter::hosted::ProviderHostedControlTransport>,
    pub(super) calls: Arc<AtomicUsize>,
    pub(super) wrong_ack: Arc<AtomicBool>,
}
pub(super) async fn receiver(
    authority: &Arc<Authority>,
) -> (Arc<Receiver>, Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
    use axum::response::IntoResponse;
    use layer3_adapter::hosted::{HostedControlTransport, ProviderHostedControlTransport};
    use layer3_adapter::provider::SandboxClient;
    use layer3_adapter::wire::{AgentRef, RequestFrame};
    let ack = Arc::new(AtomicBool::new(false));
    let flag = ack.clone();
    let wrong_ack = Arc::new(AtomicBool::new(false));
    let wrong = wrong_ack.clone();
    let expected = TransportAuthority::owner(authority.as_ref());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = address.clone();
    let router = Router::new()
        .route(
            "/v1/sandboxes/{runtime}/endpoints/{port}",
            axum::routing::get(move || {
                let endpoint = endpoint.clone();
                async move { axum::Json(json!({"endpoint":endpoint})) }
            }),
        )
        .route(
            "/runtime/agents/{agent}/manager-ownership/{action}",
            axum::routing::post(
                move |axum::extract::Path((_agent, action)): axum::extract::Path<(
                    String,
                    String,
                )>,
                      axum::Json(mut owner): axum::Json<TransportOwner>| {
                    let flag = flag.clone();
                    let count = count.clone();
                    let wrong = wrong.clone();
                    let expected = expected.clone();
                    async move {
                        if action == "revoke" {
                            assert_eq!(
                                owner, expected,
                                "revocation must carry the exact cancelled token"
                            );
                            count.fetch_add(1, Ordering::SeqCst);
                        }
                        if action == "revoke" && flag.load(Ordering::SeqCst) {
                            if wrong.load(Ordering::SeqCst) {
                                owner.epoch += 1;
                            }
                            axum::Json(OwnerAcknowledgement {
                                protocol: OWNER_PROTOCOL_VERSION,
                                owner,
                            })
                            .into_response()
                        } else {
                            axum::http::StatusCode::NOT_FOUND.into_response()
                        }
                    }
                },
            ),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let transport = Arc::new(
        ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
            &address,
            "local-fixture-only",
        ))
        .unwrap(),
    );
    let request = RequestFrame::CaptureAgent(AgentRef {
        agent_id: AgentSlot::new(authority.token.agent_id.to_string()),
    });
    let result = authority
        .run(transport.exchange(&RuntimeHandle("local-runtime".into()), request))
        .await;
    assert!(result.is_err() || result.unwrap().is_err());
    assert!(
        authority.receiver_cancelled(),
        "real failed negotiation must cancel authority"
    );
    println!(
        "AM503_RECEIVER {}",
        json!({"initial_challenge_http":404,"receiver_cancelled":true,"adapter":"ProviderHostedControlTransport"})
    );
    (
        Arc::new(Receiver {
            _transport: transport,
            calls,
            wrong_ack,
        }),
        ack,
        server,
    )
}
