use super::{
    agents_tests as agents,
    resource_access_tests::context,
    shared_agent_tests::{Authority, Reply},
};
use crate::{AgentConversationState, DelegatedCredential};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use futures_util::{FutureExt, StreamExt};
use layer2_core::{
    resource_access::*,
    sandbox::{
        store::{AgentLogStore, AgentStore, InMemorySandboxStore},
        AgentLogKind, AgentStatus, TypedFailure,
    },
    AgentId, SubjectId,
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[path = "shared_stream_live_tests.rs"]
mod live;

pub(super) struct Fixture {
    pub state: AgentConversationState,
    pub store: Arc<InMemorySandboxStore>,
    pub authority: Arc<Authority>,
    pub access: Arc<ResourceAccessService>,
    pub share: ResourceShare,
}

pub(super) async fn fixture() -> Fixture {
    let (mut state, store) = agents::conversation_fixture().await;
    let authority = Arc::new(Authority(Mutex::new(Reply::Member)));
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    state.reads = state.reads.with_resource_access(access.clone());
    agents::insert_agent(
        &store,
        agents::seeded_agent("agt_shared_stream", AgentStatus::Ready, 9, None),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: Some("private-origin".into()),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("private".into(), "private-metadata".into())].into(),
            },
            AgentLogKind::OwnerMessage {
                content: "Please review".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Review complete".into(),
            },
            AgentLogKind::SignInRequired {
                url: "https://private-sign-in.test".into(),
                code: Some("private-code".into()),
                message: None,
            },
            AgentLogKind::Usage {
                input_tokens: 5,
                output_tokens: 8,
                context_used: 13,
                context_window: None,
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "agent_turn_failed".into(),
                    detail: "private-host and private-path".into(),
                },
            },
        ],
    )
    .await;
    agents::insert_agent(
        &store,
        agents::seeded_agent("agt_sibling", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let share = access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_shared_stream".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "grant".into(),
            },
        )
        .await
        .unwrap()
        .share;
    Fixture {
        state,
        store,
        authority,
        access,
        share,
    }
}

pub(super) async fn revoke(f: &Fixture) {
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: f.share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: f.share.id.clone(),
                },
                expected_share_revision: Some(f.share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
}

fn request(subject: &str, query: &str, verified: bool) -> Request<Body> {
    let mut request = agents::request(
        Method::GET,
        &format!("/api/v1/agui/events?thread_id=agt_shared_stream&{query}"),
        subject,
        None,
        None,
    );
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(DelegatedCredential::new("request-credential".into()));
    }
    request
}

#[tokio::test]
async fn shared_replay_preserves_owner_bytes_and_hides_private_events_with_resumable_cursors() {
    for bounded in [false, true] {
        let mut f = fixture().await;
        f.state = f.state.with_bounded_replay(bounded);
        let router = crate::agui::router(f.state.clone());
        let id = AgentId("agt_shared_stream".into());
        let agent = f.store.get_agent(&id).await.unwrap().unwrap();
        let history = f.store.agent_log(&id, 0, 200).await.unwrap();
        let (_, personal) =
            agents::response(&router, request("owner-a", "follow=false", false)).await;
        let (_, verified) =
            agents::response(&router, request("owner-a", "follow=false", true)).await;
        assert_eq!(personal, verified);
        let (status, shared) =
            agents::response(&router, request("recipient", "follow=false", true)).await;
        assert_eq!(status, StatusCode::OK);
        let wire = String::from_utf8(shared.to_vec()).unwrap();
        assert!(!wire.contains("private-"));
        assert!(wire.contains("Please review") && wire.contains("Review complete"));
        assert!(wire.contains("The Agent run failed."));
        let cursors: Vec<_> = wire
            .lines()
            .filter_map(|line| line.strip_prefix("id: "))
            .collect();
        assert_eq!(cursors, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        let (status, resumed) = agents::response(
            &router,
            request("recipient", "starting_after=7&follow=false", true),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let resumed = String::from_utf8(resumed.to_vec()).unwrap();
        assert!(!resumed.contains("Review complete"));
        assert!(resumed.contains("id: 8") && resumed.contains("id: 9"));
        assert_eq!(f.store.get_agent(&id).await.unwrap().unwrap(), agent);
        assert_eq!(f.store.agent_log(&id, 0, 200).await.unwrap(), history);
    }
}

#[tokio::test]
async fn buffered_stream_frames_stop_on_revoke_while_the_owner_keeps_reading() {
    for bounded in [false, true] {
        let mut f = fixture().await;
        f.state = f.state.with_bounded_replay(bounded);
        let router = crate::agui::router(f.state.clone());
        let shared = router
            .clone()
            .oneshot(request("recipient", "follow=false", true))
            .await
            .unwrap();
        let personal = router
            .clone()
            .oneshot(request("owner-a", "follow=false", true))
            .await
            .unwrap();
        let mut shared = shared.into_body().into_data_stream();
        let mut personal = personal.into_body().into_data_stream();
        assert!(shared.next().await.unwrap().is_ok());
        assert!(personal.next().await.unwrap().is_ok());
        revoke(&f).await;
        assert!(
            shared.next().await.is_none(),
            "no pending frame may escape after revocation"
        );
        assert!(
            personal.next().await.unwrap().is_ok(),
            "independent owner survives the grant change"
        );
        assert_eq!(
            router
                .oneshot(request("recipient", "follow=false", true))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn revocation_between_large_message_chunks_cannot_acknowledge_partial_output() {
    let mut f = fixture().await;
    f.state = f.state.with_bounded_replay(true);
    let id = AgentId("agt_shared_stream".into());
    let mut agent = f.store.get_agent(&id).await.unwrap().unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 10,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Another request".into(),
        },
        AgentLogKind::AgentMessage {
            content: "retained λ output".repeat(10_000),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 10 },
    ] {
        f.store
            .append_agent_log(&id, agent.updated_at.clone(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 13;
    f.store.save_agent(revision, agent).await.unwrap();
    let router = crate::agui::router(f.state.clone());
    let response = router
        .oneshot(request("recipient", "starting_after=11&follow=false", true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    for _ in 0..2 {
        let frame = body.next().await.unwrap().unwrap();
        let frame = String::from_utf8_lossy(&frame);
        assert!(!frame.contains("id:") && !frame.contains("TEXT_MESSAGE_END"));
    }
    revoke(&f).await;
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn buffered_and_idle_streams_close_at_membership_deadline_and_reconnect_checks_current_role()
{
    for bounded in [false, true] {
        for query in ["follow=false", "starting_after=9&follow=true"] {
            let mut f = fixture().await;
            f.state = f.state.with_bounded_replay(bounded);
            let router: Router = crate::agui::router(f.state.clone());
            let response = router
                .clone()
                .oneshot(request("recipient", query, true))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body().into_data_stream();
            if query == "follow=false" {
                assert!(body.next().await.unwrap().is_ok());
            } else {
                assert!(body.next().now_or_never().is_none());
            }
            *f.authority.0.lock().unwrap() = Reply::Missing;
            tokio::time::pause();
            tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
            assert!(body.next().await.is_none());
            tokio::time::resume();
            assert_eq!(
                router
                    .clone()
                    .oneshot(request("recipient", "follow=false", true))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
            *f.authority.0.lock().unwrap() = Reply::Outage;
            assert_eq!(
                router
                    .clone()
                    .oneshot(request("recipient", "follow=false", true))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_eq!(
                router
                    .oneshot(request("owner-a", "follow=false", true))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
    }
}
