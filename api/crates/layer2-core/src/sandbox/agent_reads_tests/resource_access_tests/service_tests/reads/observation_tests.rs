use super::*;
use crate::sandbox::agent_observation::*;
use std::sync::atomic::AtomicBool;

type RefreshGate = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

struct Facts {
    store: Arc<InMemorySandboxStore>,
    available: AtomicBool,
    stalled: AtomicBool,
    epoch: AtomicUsize,
    refresh_gate: tokio::sync::Mutex<Option<RefreshGate>>,
}
#[async_trait::async_trait]
impl AgentObservationStore for Facts {
    fn ready(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
    async fn source_generation(&self, agent: &Agent) -> Result<Option<Vec<u8>>, SandboxStoreError> {
        Ok(Some(serde_json::to_vec(agent).unwrap()))
    }
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<Result<ResourceAccessSnapshot, AgentReadError>>, AgentReadError> {
        if self.stalled.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let mut result = Vec::new();
        for token in tokens {
            if token.epoch != self.epoch.load(Ordering::SeqCst) as u64 {
                result.push(Err(AgentReadError::Unavailable("stale read epoch".into())));
            } else {
                result.push(
                    self.store
                        .resource_access_snapshot(&ResourceTarget::Agent {
                            agent_id: token.agent_id.clone(),
                        })
                        .await
                        .map_err(|error| AgentReadError::Access(error.into())),
                );
            }
        }
        Ok(result)
    }
    async fn refresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        if let Some((entered, resume)) = self.refresh_gate.lock().await.take() {
            entered.notify_one();
            resume.notified().await;
        }
        Ok(tokens
            .iter()
            .cloned()
            .zip(self.fresh(tokens).await?)
            .map(|(token, snapshot)| AgentObservationRefresh {
                token,
                until: tokio::time::Instant::now() + Duration::from_secs(15),
                snapshot: Some(snapshot),
            })
            .collect())
    }
}
fn custody(agent: &Agent, epoch: u64) -> Arc<AgentObservationAuthority> {
    AgentObservationAuthority::new(
        AgentObservationToken {
            agent_id: agent.id.clone(),
            created_at: agent.created_at.clone(),
            instance_id: "replica-a".into(),
            epoch,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
    )
}
fn observer(
    reads: &Arc<AgentReadService>,
    store: Arc<InMemorySandboxStore>,
) -> (
    Arc<AgentReadService>,
    Arc<AgentObservationService>,
    Arc<Facts>,
) {
    let facts = Arc::new(Facts {
        store,
        available: AtomicBool::new(true),
        stalled: AtomicBool::new(false),
        epoch: AtomicUsize::new(1),
        refresh_gate: tokio::sync::Mutex::new(None),
    });
    let observation = AgentObservationService::new(reads.clone(), facts.clone());
    (
        reads.with_observation(observation.clone()),
        observation,
        facts,
    )
}

#[tokio::test]
async fn observation_cache_never_shares_authority_or_private_fields_and_revocation_closes_before_release(
) {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let share = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "observe"),
        )
        .await
        .unwrap()
        .share;
    let reads = service(store.clone()).with_resource_access(Arc::new(access.clone()));
    let (reads, _, _) = observer(&reads, store);
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let mut recipient = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let (owner_watch, mut shared_watch) = observation_scope(custody(&agent, 1), async {
        (
            reads.observe_event_stream(&mut owner).unwrap().unwrap(),
            reads.observe_event_stream(&mut recipient).unwrap().unwrap(),
        )
    })
    .await;
    let a = reads
        .event_stream_window(&owner, 0, None, 200)
        .await
        .unwrap();
    let b = reads
        .event_stream_window(&recipient, 0, None, 200)
        .await
        .unwrap();
    assert_eq!(a.entries.len(), b.entries.len());
    assert!(a.entries[0].kind.is_some());
    assert!(b.entries[0].kind.is_none());
    for entry in b.entries {
        assert!(!format!("{entry:?}").contains("private-"));
    }
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(reads.validate_event_stream(&recipient).await.is_err());
    reads.validate_event_stream(&owner).await.unwrap();
    assert!(shared_watch.changed().await.is_err());
    assert!(!shared_watch.current());
    assert!(owner_watch.current());
}

#[tokio::test]
async fn observation_custody_loss_stale_epoch_and_wrong_incarnation_refuse_without_polling_another_agent(
) {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, facts) = observer(&reads, store);
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    assert!(reads.observe_event_stream(&mut owner).is_err());
    let mut wrong = agent.clone();
    wrong.created_at = "2000-01-01T00:00:00Z".into();
    assert!(observation_scope(custody(&wrong, 1), async {
        reads.observe_event_stream(&mut owner)
    })
    .await
    .is_err());
    let watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut owner).unwrap().unwrap()
    })
    .await;
    reads.validate_event_stream(&owner).await.unwrap();
    facts.epoch.store(2, Ordering::SeqCst);
    assert!(reads.validate_event_stream(&owner).await.is_err());
    facts.available.store(false, Ordering::SeqCst);
    assert!(!watch.current());
    assert!(reads.validate_event_stream(&owner).await.is_err());
}

#[tokio::test]
async fn observation_lag_coalesces_and_local_deadline_timeout_do_not_renew_membership() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "observe"),
        )
        .await
        .unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, facts) = observer(&reads, store);
    let mut auth = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let mut watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut auth).unwrap().unwrap()
    })
    .await;
    let deadline = auth.revalidation_deadline();
    tokio::time::pause();
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(
        watch.changed().await.unwrap().head,
        agent.conversation.last_seq
    );
    facts.stalled.store(true, Ordering::SeqCst);
    assert!(reads.validate_event_stream(&auth).await.is_err());
    tokio::time::advance(MAX_MEMBERSHIP_AGE).await;
    assert!(watch.changed().await.is_err());
    assert_eq!(auth.revalidation_deadline(), deadline);
    assert!(authority.calls.load(Ordering::SeqCst) > 1);
}

#[tokio::test]
async fn observation_single_flight_eviction_and_cancelled_subscribers_preserve_results() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, observation, _) = observer(&reads, store);
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut owner).unwrap().unwrap()
    })
    .await;
    let loads = AtomicUsize::new(0);
    let load = || async {
        loads.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(Some(Vec::new()))
    };
    let requests = (0..100).map(|_| async {
        observation
            .source_page(&agent, (0, 1, 1, true), load)
            .await
            .map(|_| ())
    });
    assert!(futures_util::future::join_all(requests)
        .await
        .iter()
        .all(Result::is_ok));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    for coordinate in 1..=256 {
        observation
            .source_page(&agent, (coordinate, coordinate, 1, true), load)
            .await
            .unwrap();
    }
    observation
        .source_page(&agent, (0, 1, 1, true), load)
        .await
        .unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 258);
    drop(watch);
    assert!(reads.validate_event_stream(&owner).await.is_err());
}

#[tokio::test]
async fn observation_membership_revocation_closes_before_the_next_frame_without_waiting_for_expiry()
{
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "observe"),
        )
        .await
        .unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, _) = observer(&reads, store);
    let mut auth = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let _watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut auth).unwrap().unwrap()
    })
    .await;
    reads.validate_event_stream(&auth).await.unwrap();
    *authority.reply.lock().unwrap() = Reply::Missing;
    assert!(auth.revalidation_deadline().unwrap() > tokio::time::Instant::now());
    assert!(reads.validate_event_stream(&auth).await.is_err());
}

#[tokio::test]
async fn observation_periodic_catchup_recovers_a_dropped_hint_without_losing_the_cursor() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, _) = observer(&reads, store.clone());
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let mut watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut owner).unwrap().unwrap()
    })
    .await;
    let mut cursor = AgentHistoryCursor::default();
    reads
        .event_stream_page(&owner, &mut cursor, None)
        .await
        .unwrap();
    let after = cursor.sequence();
    let appended = store
        .append_agent_log(
            &agent.id,
            "2026-11-16T00:00:00Z".into(),
            AgentLogKind::ModeChanged {
                mode: "code".into(),
            },
        )
        .await
        .unwrap();
    let mut committed = agent.clone();
    committed.conversation.last_seq = appended.seq;
    committed.revision += 1;
    store.save_agent(agent.revision, committed).await.unwrap();
    // No hint recorder is installed in this fixture.
    assert_eq!(watch.changed().await.unwrap().head, appended.seq);
    let page = reads
        .event_stream_page(&owner, &mut cursor, None)
        .await
        .unwrap();
    assert_eq!(page.history.len(), 1);
    assert_eq!(page.history[0].seq, after + 1);
    assert_eq!(cursor.sequence(), appended.seq);
}

#[tokio::test]
async fn observation_subscriber_capacity_and_oversized_loads_remain_bounded() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, observation, _) = observer(&reads, store);
    let owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let mut watches = observation_scope(custody(&agent, 1), async {
        let mut watches = Vec::new();
        for _ in 0..MAX_OBSERVERS {
            watches.push(
                reads
                    .observe_event_stream(&mut owner.clone())
                    .unwrap()
                    .unwrap(),
            );
        }
        assert!(reads.observe_event_stream(&mut owner.clone()).is_err());
        watches
    })
    .await;
    watches.pop();
    let watch = observation_scope(custody(&agent, 1), async {
        reads
            .observe_event_stream(&mut owner.clone())
            .unwrap()
            .unwrap()
    })
    .await;
    // An oversized uncached result holds the same single-flight guard through
    // its separately bounded visitor path; concurrent loads wait for its drop.
    let first = observation
        .source_page(&agent, (0, 1, 1, true), || async { Ok(None) })
        .await
        .unwrap();
    let calls = AtomicUsize::new(0);
    let second = observation.source_page(&agent, (0, 1, 1, true), || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    });
    tokio::pin!(second);
    assert!(tokio::time::timeout(Duration::from_millis(10), &mut second)
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(first);
    assert!(second.await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(watch);
    drop(watches);
}

#[tokio::test]
async fn observation_release_queue_refuses_overload_without_renewing_timed_out_requests() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, facts) = observer(&reads, store);
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let _watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut owner).unwrap().unwrap()
    })
    .await;
    facts.stalled.store(true, Ordering::SeqCst);
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let results = futures_util::future::join_all((0..MAX_OBSERVERS * 2).map(|_| async {
        let result = reads.validate_event_stream(&owner).await;
        (result, started.elapsed())
    }))
    .await;
    assert!(results.iter().all(|(result, _)| result.is_err()));
    assert!(
        results.iter().any(|(_, elapsed)| elapsed.is_zero()),
        "overload is refused without an unbounded wait queue"
    );
    // Tokio rounds timer delivery up to its next millisecond tick. The
    // request still uses its original 15-second deadline, never a renewed one.
    assert!(started.elapsed() <= Duration::from_secs(15) + Duration::from_millis(1));
}

#[tokio::test]
async fn observation_commit_hints_wake_before_the_periodic_slot_and_cancel_cleanly() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, observation, _) = observer(&reads, store.clone());
    let updates = crate::sandbox::DurableAgentUpdateRecorder::new(store);
    observation.install_hints(updates.clone()).unwrap();
    assert!(observation.install_hints(updates.clone()).is_err());
    let mut owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let mut watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut owner).unwrap().unwrap()
    })
    .await;
    let native = crate::agent_ownership::Authority::new(
        crate::agent_ownership::OwnershipToken {
            protocol: crate::agent_ownership::PROTOCOL_VERSION,
            agent_id: agent.id.clone(),
            instance_id: "writer".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
    );
    for _ in 0..1000 {
        assert!(updates.notify_owned_head(&agent.id, &native));
    }
    let changed = tokio::time::timeout(Duration::from_millis(500), watch.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.head, agent.conversation.last_seq);
    drop(watch);
    assert!(reads.validate_event_stream(&owner).await.is_err());
    assert!(updates.notify_owned_head(&agent.id, &native));
}

#[tokio::test]
async fn observation_join_during_periodic_io_is_not_denied_by_an_earlier_cohort() {
    let (store, _, access, _) = setup(Reply::Missing).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let (reads, _, facts) = observer(&reads, store);
    let mut first = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let mut second = first.clone();
    let mut first_watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut first).unwrap().unwrap()
    })
    .await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    *facts.refresh_gate.lock().await = Some((entered.clone(), resume.clone()));
    entered.notified().await;
    let second_watch = observation_scope(custody(&agent, 1), async {
        reads.observe_event_stream(&mut second).unwrap().unwrap()
    })
    .await;
    resume.notify_one();
    assert_eq!(
        first_watch.changed().await.unwrap().head,
        agent.conversation.last_seq
    );
    reads.validate_event_stream(&second).await.unwrap();
    assert!(first_watch.current() && second_watch.current());
}
