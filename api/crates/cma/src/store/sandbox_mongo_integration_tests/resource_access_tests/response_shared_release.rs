use super::*;
use axum::{body::Body, http::Request};
use fixture::context;
use layer2_core::sandbox::agent_observation::observation_scope;
use tower::ServiceExt;

struct Membership;
#[async_trait::async_trait]
impl OrganizationAuthority for Membership {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if caller.subject.as_str() != "response-reader" || org.as_str() != "org" {
            return Err(MembershipError::NotMember);
        }
        let mut value = context(caller.subject.clone());
        let now = std::time::SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("response reads do not enumerate organizations")
    }
}

fn public_request(path: &str, subject: SubjectId) -> Request<Body> {
    let context = context(subject);
    let mut request = Request::get(path).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            context.caller.subject.clone(),
        ));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "response-fixture".into(),
        ));
    request
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn mongo_share_revocation_blocks_the_next_buffered_response_frame_without_a_failed_outcome() {
    let (f, id) = retained(1_000).await;
    let source = Arc::new(
        f.store
            .clone()
            .with_access_snapshot_session(true)
            .with_bounded_response_reads(true),
    );
    let access = Arc::new(ResourceAccessService::new(
        source.clone(),
        Arc::new(Membership),
    ));
    let base = f
        .reads(true)
        .with_resource_access(access.clone())
        .with_response_logs(source.response_log_reader());
    let native = Arc::new(MongoAgentOwnership::open(&f.client, &f.name).await.unwrap());
    let coordinator = OwnershipCoordinator::register(
        native,
        Member {
            instance_id: "buffered-output".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap();
    let store = MongoAgentObservation::open(source.clone(), coordinator)
        .await
        .unwrap();
    let custody = store
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap()
        .authority;
    let service = AgentObservationService::new(base.clone(), store);
    let reads = base.with_observation(service);
    let commands = AgentCommandService::new(source.clone(), access, Arc::new(fixture::Authority));
    let app = layer1_public::responses::v2::router(
        layer1_public::responses::v2::ResponsesV2State::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            commands,
            reads,
        )
        .with_paged_events(true),
    );
    let target = ResourceTarget::Agent {
        agent_id: f.agent.id.clone(),
    };
    let snapshot = source.resource_access_snapshot(&target).await.unwrap();
    let share = source
        .mutate_share(
            crate::store::sandbox_mongo_integration_tests::resource_access_tests::mutation(
                &snapshot,
                "output-release",
                ShareMode::View,
            ),
        )
        .await
        .unwrap()
        .share;
    let path = format!("/api/v2/responses/{id}/events");
    let owner = observation_scope(
        custody.clone(),
        app.clone()
            .oneshot(public_request(&path, f.agent.owner.clone())),
    )
    .await
    .unwrap();
    let mut owner_body = owner.into_body().into_data_stream();
    assert_eq!(
        events(&owner_body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    let shared = observation_scope(
        custody.clone(),
        app.clone()
            .oneshot(public_request(&path, SubjectId::new("response-reader"))),
    )
    .await
    .unwrap();
    assert_eq!(shared.status(), reqwest::StatusCode::OK);
    let mut body = shared.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    // Deliberately stop polling the body with replay frames still buffered.
    // The real Mongo policy change must win before the next body poll.
    let snapshot = source.resource_access_snapshot(&target).await.unwrap();
    let mut revoke = crate::store::sandbox_mongo_integration_tests::resource_access_tests::mutation(
        &snapshot,
        "output-revoke",
        ShareMode::View,
    );
    revoke.change = ShareChange::Revoke { share_id: share.id };
    revoke.expected_share_revision = Some(share.revision);
    source.mutate_share(revoke).await.unwrap();
    let refused = body.next().await.unwrap().unwrap();
    assert_eq!(events(&refused)[0]["type"], "error");
    assert!(!String::from_utf8_lossy(&refused).contains("id: "));
    assert!(body.next().await.is_none());
    let owner_next = owner_body.next().await.unwrap().unwrap();
    assert_ne!(events(&owner_next)[0]["type"], "error");
    assert!(String::from_utf8_lossy(&owner_next).contains("id: 1"));
    let denied = observation_scope(
        custody,
        app.oneshot(public_request(&path, SubjectId::new("response-reader"))),
    )
    .await
    .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
    drop(body);
    drop(owner_body);
    f.close().await;
}
