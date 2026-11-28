use super::*;
use layer2_core::sandbox::{agent_observation::*, Agent, AgentReadError, AgentReadService};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) struct Facts {
    store: Arc<InMemorySandboxStore>,
    commands: Arc<dyn AgentCommandStore>,
    pub generation: AtomicUsize,
    ready: AtomicBool,
    revoke_on_fresh: std::sync::Mutex<Option<(usize, Arc<AgentObservationAuthority>)>>,
}
#[async_trait]
impl AgentObservationStore for Facts {
    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
    async fn commands(
        &self,
        ids: &[layer2_core::ids::AgentResponseId],
    ) -> Result<Vec<Result<AdmittedAgentCommand, AgentReadError>>, AgentReadError> {
        let mut rows = Vec::new();
        for id in ids {
            rows.push(
                self.commands
                    .get_response_command(id)
                    .await
                    .map_err(|_| AgentReadError::NotFound)
                    .and_then(|record| record.ok_or(AgentReadError::NotFound)),
            );
        }
        Ok(rows)
    }
    async fn source_generation(
        &self,
        agent: &Agent,
    ) -> Result<Option<Vec<u8>>, layer2_core::sandbox::store::SandboxStoreError> {
        Ok(Some(
            serde_json::to_vec(&(agent, self.generation.load(Ordering::SeqCst))).unwrap(),
        ))
    }
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<Result<ResourceAccessSnapshot, AgentReadError>>, AgentReadError> {
        let mut rows = Vec::new();
        for token in tokens {
            rows.push(
                self.store
                    .resource_access_snapshot(&ResourceTarget::Agent {
                        agent_id: token.agent_id.clone(),
                    })
                    .await
                    .map_err(|error| AgentReadError::Access(error.into())),
            );
        }
        let revoke = {
            let mut armed = self.revoke_on_fresh.lock().unwrap();
            if let Some((remaining, _)) = armed.as_mut() {
                *remaining -= 1;
                if *remaining == 0 {
                    armed.take().map(|(_, authority)| authority)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(authority) = revoke {
            authority.revoke();
        }
        Ok(rows)
    }
    async fn refresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        Ok(tokens
            .iter()
            .cloned()
            .zip(self.fresh(tokens).await?)
            .map(|(token, snapshot)| AgentObservationRefresh {
                token,
                snapshot: Some(snapshot),
                until: tokio::time::Instant::now() + Duration::from_secs(15),
            })
            .collect())
    }
}
pub(super) fn install(
    reads: Arc<AgentReadService>,
    store: Arc<InMemorySandboxStore>,
    agent: &Agent,
) -> (
    Arc<AgentReadService>,
    Arc<AgentObservationAuthority>,
    Arc<Facts>,
) {
    install_with_commands(reads, store.clone(), store, agent)
}

pub(super) fn install_with_commands(
    reads: Arc<AgentReadService>,
    store: Arc<InMemorySandboxStore>,
    commands: Arc<dyn AgentCommandStore>,
    agent: &Agent,
) -> (
    Arc<AgentReadService>,
    Arc<AgentObservationAuthority>,
    Arc<Facts>,
) {
    let facts = Arc::new(Facts {
        store,
        commands,
        generation: AtomicUsize::new(1),
        ready: AtomicBool::new(true),
        revoke_on_fresh: std::sync::Mutex::new(None),
    });
    let service = AgentObservationService::new(reads.clone(), facts.clone());
    let custody = AgentObservationAuthority::new(
        AgentObservationToken {
            agent_id: agent.id.clone(),
            created_at: agent.created_at.clone(),
            instance_id: "output-fixture".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
    );
    (reads.with_observation(service), custody, facts)
}

#[tokio::test]
async fn command_only_rejection_invalidates_shared_queued_projection_without_log_change() {
    let f = fixture_mode(None, true, true).await;
    let response = create(&f, false).await;
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(events(&first)[0]["type"], "response.created");
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(3), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(events(&next)[0]["type"], "response.failed");
    assert_eq!(events(&next)[0]["sequence_number"], 1);
    assert!(body.next().await.is_none());
    assert_eq!(agent, f.store.get_agent(&agent.id).await.unwrap().unwrap());
}

#[tokio::test]
async fn personal_response_resumes_at_the_same_cursor_after_shared_custody_is_lost() {
    let (f, observer) = fixture_with_events_and_standing(
        None,
        true,
        true,
        Arc::new(layer2_core::account_deletion::AvailableAccounts),
    )
    .await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(events(&first)[0]["type"], "response.created");
    assert_eq!(events(&first)[0]["sequence_number"], 0);

    observer.unwrap().0.revoke();
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(4), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let observed = events(&frame);
    assert_eq!(observed[0]["type"], "response.failed");
    assert_eq!(observed[0]["sequence_number"], 1);
    assert_eq!(observed[0]["response"]["id"], id);
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn personal_replay_keeps_shared_subscription_refusals() {
    let (f, observer) = fixture_with_events_and_standing(
        None,
        true,
        true,
        Arc::new(layer2_core::account_deletion::AvailableAccounts),
    )
    .await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);

    observer.unwrap().1.ready.store(false, Ordering::SeqCst);
    let response = f
        .router
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("x-cma-response-id"));
}

#[tokio::test]
async fn cached_projection_never_shares_metadata_or_survives_revocation_under_backpressure() {
    let f = fixture_mode(None, true, true).await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let share = f
        .access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "shared-projection".into(),
            },
        )
        .await
        .unwrap()
        .share;
    let path = format!("/api/v2/responses/{id}");
    let (_, owner) = read_json(&f.router, Method::GET, &path, "owner-a", None).await;
    assert_ne!(owner["metadata"], json!({}));
    let (_, shared) = read_json(&f.router, Method::GET, &path, "reader", None).await;
    assert_eq!(shared["metadata"], json!({}));
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("{path}/events"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-projection".into(),
            },
        )
        .await
        .unwrap();
    let refused = body.next().await.unwrap().unwrap();
    assert_eq!(events(&refused)[0]["type"], "error");
    assert!(!String::from_utf8_lossy(&refused).contains("id: "));
    assert!(body.next().await.is_none());
    assert_eq!(
        read_json(&f.router, Method::GET, &path, "reader", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        read_json(&f.router, Method::GET, &path, "unrelated", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn queued_snapshot_final_custody_fence_keeps_existing_durable_recovery() {
    let (f, observer) = fixture_with_events_and_standing(
        None,
        true,
        true,
        Arc::new(layer2_core::account_deletion::AvailableAccounts),
    )
    .await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (custody, facts) = observer.unwrap();
    // First refresh: command/header, unbound snapshot, snapshot release fence.
    *facts.revoke_on_fresh.lock().unwrap() = Some((3, custody));
    let mut body = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(4), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        facts.revoke_on_fresh.lock().unwrap().is_none(),
        "release fence injected"
    );
    let observed = events(&frame);
    assert_eq!(
        observed[0]["type"], "response.created",
        "unbound capture custody loss must rebuild: {observed:?}"
    );
    assert_eq!(observed[0]["sequence_number"], 0);
    assert!(observed.iter().all(|event| event["type"] != "error"));
}
