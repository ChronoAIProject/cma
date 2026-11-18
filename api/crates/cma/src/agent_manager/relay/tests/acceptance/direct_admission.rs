//! Only the two durable-control fixture handlers admit without an owner.
use super::*;
use axum::response::Response;

pub(super) async fn accept(
    State(probe): State<Arc<Probe>>,
    context: Option<Extension<layer2_core::resource_access::CallerContext>>,
    peer: Option<Extension<http::VerifiedPeer>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Keep the shared synchronous-effect probe owner-bound. This handler
    // models the serving admission check after the relay has been bypassed.
    let Some(Extension(context)) = context else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let id = AgentId("admission-probe".into());
    if let Err(status) = public_access::PublicAuthorization::authorize(
        probe.as_ref(),
        public_access::PublicRequest {
            id: &id,
            method: &method,
            uri: &uri,
            headers: &headers,
            context: &context,
            credential: None,
        },
    )
    .await
    {
        return status.into_response();
    }
    let owner = layer2_core::agent_ownership::execution_owner();
    probe.effects.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::ACCEPTED,
        axum::Json(json!({
            "owner": owner.as_ref().map(|token| &token.instance_id),
            "epoch": owner.as_ref().map(|token| token.epoch),
            "relayed": peer.is_some(),
            "uri": uri.to_string(),
            "body": body.to_vec(),
            "request": headers.get("x-request-id").unwrap().to_str().unwrap(),
            "idempotency": headers.get("idempotency-key").unwrap().to_str().unwrap(),
        })),
    )
        .into_response()
}

#[tokio::test]
async fn ownerless_stop_and_delete_admit_on_each_ingress_without_claiming() {
    let (store, instances, probes) = replicas().await;
    for (method, suffix, name) in [
        (Method::POST, "/stop", "stop"),
        (Method::DELETE, "", "delete"),
    ] {
        let id = AgentId(format!("agt-ownerless-{name}"));
        for (index, instance) in instances.iter().enumerate() {
            let url = format!("{}/api/v1/agents/{id}{suffix}", instance.address);
            let response = client()
                .request(method.clone(), &url)
                .headers(headers())
                .body(b"{}".to_vec())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let accepted: Value = response.json().await.unwrap();
            assert!(accepted["owner"].is_null());
            assert!(accepted["epoch"].is_null());
            assert_eq!(accepted["relayed"], false);
            assert_eq!(accepted["uri"], format!("/api/v1/agents/{id}{suffix}"));
            assert_eq!(accepted["body"], json!(b"{}".to_vec()));
            assert_eq!(accepted["request"], "acceptance-request");
            assert_eq!(accepted["idempotency"], "acceptance-operation");
            assert!(store.owner(&id).await.unwrap().is_none());
            assert_eq!(
                client()
                    .request(method.clone(), &url)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            probes[index].allowed.store(false, Ordering::SeqCst);
            assert_eq!(
                client()
                    .request(method.clone(), &url)
                    .headers(headers())
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
            probes[index].allowed.store(true, Ordering::SeqCst);
        }
    }
    for probe in probes {
        assert_eq!(probe.effects.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn public_control_with_presented_proof_retains_owner_and_replay_checks() {
    let (store, instances, probes) = replicas().await;
    for (method, suffix, name) in [
        (Method::POST, "/stop", "stop"),
        (Method::DELETE, "", "delete"),
    ] {
        let id = AgentId(format!("agt-proof-{name}"));
        let (owner, authority) = acquire(&store, &instances, &id).await;
        let uri: Uri = format!("/api/v1/agents/{id}{suffix}").parse().unwrap();
        let sign = |token: &OwnershipToken| {
            instances[1 - owner]
                .relay
                .authentication
                .sign(
                    &instances[1 - owner].coordinator.instance_id,
                    token,
                    &method,
                    &uri,
                    &headers(),
                    b"{}",
                )
                .unwrap()
        };
        let url = format!("{}{uri}", instances[owner].address);
        let proof = sign(&authority.token);
        for (sent, expected) in [
            (b"changed".as_slice(), StatusCode::FORBIDDEN),
            (b"{}".as_slice(), StatusCode::ACCEPTED),
            (b"{}".as_slice(), StatusCode::FORBIDDEN),
        ] {
            let response = client()
                .request(method.clone(), &url)
                .headers(headers())
                .header(PROOF, &proof)
                .body(sent.to_vec())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::ACCEPTED {
                let body: Value = response.json().await.unwrap();
                assert_eq!(body["owner"], authority.token.instance_id);
                assert_eq!(body["epoch"], authority.token.epoch);
                assert_eq!(body["relayed"], true);
            }
        }
        let mut stale = authority.token.clone();
        stale.epoch += 1;
        assert_eq!(
            client()
                .request(method.clone(), &url)
                .headers(headers())
                .header(PROOF, sign(&stale))
                .body(b"{}".to_vec())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            client()
                .request(method.clone(), &url)
                .headers(headers())
                .header(PROOF, "forged")
                .body(b"{}".to_vec())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let fresh = sign(&authority.token);
        assert_eq!(
            client()
                .request(method.clone(), &url)
                .headers(headers())
                .header(PROOF, &fresh)
                .header(PROOF, &fresh)
                .body(b"{}".to_vec())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        probes
            .iter()
            .map(|p| p.effects.load(Ordering::SeqCst))
            .sum::<usize>(),
        2
    );
}
