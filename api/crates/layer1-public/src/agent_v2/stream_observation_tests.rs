use super::*;
use crate::observation_test_support::{attach, custody};
use layer2_core::sandbox::agent_observation::observation_scope;

#[tokio::test]
async fn shared_observation_fences_each_buffered_frame_and_isolates_the_owner() {
    let mut f = fixture(true, true).await;
    let share = share(&f).await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    (f.state.reads, _) = attach(&f.state.reads, f.store.clone());
    f.router = crate::agent_v2::router(f.state.clone());
    let response = observation_scope(
        custody(&agent),
        f.router.clone().oneshot(request(
            Method::GET,
            &format!("{EVENTS}?follow=false"),
            "reader",
            None,
            None,
        )),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&first)
        .unwrap()
        .contains("\"kind\":null"));
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "shared-observation-revoke".into(),
            },
        )
        .await
        .unwrap();
    assert!(body.next().await.is_none());
    let response = observation_scope(
        custody(&agent),
        f.router.oneshot(request(
            Method::GET,
            &format!("{EVENTS}?follow=false"),
            "owner-a",
            None,
            None,
        )),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let wire = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert!(std::str::from_utf8(&wire)
        .unwrap()
        .contains("cma.agent.event"));
}

#[tokio::test]
async fn shared_live_frame_requires_fresh_authority_even_after_batch_validation() {
    let mut f = fixture(true, true).await;
    let share = share(&f).await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    (f.state.reads, _) = attach(&f.state.reads, f.store.clone());
    let request = request(Method::GET, EVENTS, "reader", None, None);
    let principal = request.extensions().get::<CallerContext>().unwrap().clone();
    let auth = AuthenticatedResourceCaller {
        context: principal,
        credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
    };
    let mut open = observation_scope(
        custody(&agent),
        open(
            f.state,
            auth,
            agent.id.clone(),
            Some("follow=true".into()),
            HeaderMap::new(),
        ),
    )
    .await
    .unwrap();
    assert!(
        open.state.event_wake,
        "shared observation must work while the older wake flag is off"
    );
    open.live_batch_needs_validation = false;
    let frame = Frame {
        event: Event::default().data("buffered live"),
        live: true,
        fragment: Some(
            AgentLiveFragment {
                agent_id: agent.id,
                runtime_generation: 1,
                turn_sequence: 3,
                after_seq: agent.conversation.last_seq,
                message_id: "live".into(),
                kind: AgentLiveFragmentKind::Thought,
                delta: "private live".into(),
            }
            .into(),
        ),
    };
    assert!(open.release(&frame).await);
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "shared-live-revoke".into(),
            },
        )
        .await
        .unwrap();
    assert!(!open.release(&frame).await);
    assert!(open.release_failed);
}
