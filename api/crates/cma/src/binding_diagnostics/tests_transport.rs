use super::*;

#[tokio::test]
async fn transport_bounds_streamed_bytes_without_waiting_for_an_open_body() {
    let members = Arc::new(Members::default());
    let a = service("a", members.clone(), vec![]);
    let upstream = server(Router::new().route(
        PATH,
        get(|| async {
            let chunks = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(vec![
                    b'x';
                    MAX_BODY + 1
                ])]);
            let body = Body::from_stream(chunks.chain(futures_util::stream::pending()));
            axum::response::Response::builder()
                .header(authentication::PROOF_HEADER, "AA")
                .body(body)
                .unwrap()
        }),
    ))
    .await;
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", &upstream.address),
    ];
    let view = tokio::time::timeout(
        Duration::from_secs(1),
        a.observe(&SubjectId("operator".into())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(view.unavailable_reporters, 1);
    assert_eq!(view.reports.len(), 1);
    assert_eq!(a.peer_slots.available_permits(), 4);
}

#[tokio::test]
async fn transport_rejects_valid_json_when_signed_bytes_were_changed() {
    let members = Arc::new(Members::default());
    let a = service("a", members.clone(), vec![]);
    let b = service("b", members.clone(), vec![hold("protected")]);
    let upstream = server(Router::new().route(
        PATH,
        get(move |request: Request<Body>| {
            let b = b.clone();
            async move {
                let response = b.router().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let (mut parts, body) = response.into_parts();
                parts.headers.remove(axum::http::header::CONTENT_LENGTH);
                let mut bytes = to_bytes(body, MAX_BODY).await.unwrap().to_vec();
                bytes.push(b' ');
                assert!(serde_json::from_slice::<BindingHoldSample>(&bytes).is_ok());
                axum::response::Response::from_parts(parts, Body::from(bytes))
            }
        }),
    ))
    .await;
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", &upstream.address),
    ];
    let view = a.observe(&SubjectId("operator".into())).await.unwrap();
    assert_eq!(view.unavailable_reporters, 1);
    assert_eq!(view.reports.len(), 1);
    assert_eq!(view.observed_sandboxes, 0);
}

#[tokio::test]
async fn aggregate_rechecks_membership_after_a_valid_signed_reply() {
    let members = Arc::new(Members::default());
    let a = service("a", members.clone(), vec![]);
    let b = service("b", members.clone(), vec![hold("held")]);
    let upstream = server(Router::new().route(
        PATH,
        get({
            let members = members.clone();
            move |request: Request<Body>| {
                let b = b.clone();
                let members = members.clone();
                async move {
                    let response = b.router().oneshot(request).await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    members
                        .values
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .find(|member| member.instance_id == "b")
                        .unwrap()
                        .draining = true;
                    response
                }
            }
        }),
    ))
    .await;
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", &upstream.address),
    ];
    members
        .owners
        .lock()
        .unwrap()
        .insert(SandboxId("held".into()), SubjectId("owner".into()));
    let view = a.observe(&SubjectId("operator".into())).await.unwrap();
    assert_eq!(view.unavailable_reporters, 1);
    assert_eq!(view.reports.len(), 1);
    assert_eq!(view.observed_sandboxes, 0);
}

struct DepartingLocal {
    members: Arc<Members>,
}
impl LocalSource for DepartingLocal {
    fn sample(&self) -> BindingHoldSample {
        self.members
            .values
            .lock()
            .unwrap()
            .retain(|member| member.instance_id != "b");
        Local(vec![]).sample()
    }
}

#[tokio::test]
async fn peer_rechecks_both_members_after_sampling_and_rejects_query_or_duplicate_proofs() {
    let members = Arc::new(Members::default());
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", "http://b.example.test/"),
    ];
    let a = service("a", members.clone(), vec![]);
    let b = BindingDiagnostics::assemble(
        "b".into(),
        Box::new(|| true),
        members.clone(),
        Arc::new(DepartingLocal { members }),
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[7; 32]),
    )
    .unwrap();
    for (query, duplicate) in [(true, false), (false, true), (false, false)] {
        let (_, proof) = a.authentication.request("a", "b", "operator").unwrap();
        let mut request = Request::builder()
            .uri(if query {
                format!("{PATH}?extra=1")
            } else {
                PATH.into()
            })
            .header(authentication::PROOF_HEADER, &proof);
        if duplicate {
            request = request.header(authentication::PROOF_HEADER, &proof);
        }
        let response = b
            .router()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
