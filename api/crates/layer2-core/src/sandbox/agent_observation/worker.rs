use super::*;

const RELEASE_BATCH_WINDOW: Duration = Duration::from_millis(6);

pub(super) async fn run(
    service: Weak<AgentObservationService>,
    mut receive: mpsc::Receiver<Fence>,
) {
    let mut tick =
        tokio::time::interval_at(Instant::now() + OBSERVATION_INTERVAL, OBSERVATION_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = tick.tick() => {
                let Some(service) = service.upgrade() else { return };
                periodic(&service, false).await;
                // Start the next interval after completed work. Timer jitter
                // cannot repeatedly arrive just before the persisted slot.
                tick.reset_after(OBSERVATION_INTERVAL);
            }
            _ = async {
                if let Some(service) = service.upgrade() { service.wake.notified().await; }
            } => {
                let Some(service) = service.upgrade() else { return };
                periodic(&service, true).await;
            }
            first = receive.recv() => {
                let Some(first) = first else { return };
                let Some(service) = service.upgrade() else { return };
                let mut cohort = vec![first];
                while cohort.len() < MAX_OBSERVATION_COHORT {
                    match receive.try_recv() { Ok(request) => cohort.push(request), Err(_) => break }
                }
                // Other readers can queue their next fence before the fresh
                // snapshot. Dispatch a lone subscriber or a full batch now.
                let multiple_readers = service.registry.lock().unwrap().subscribers > 1;
                if multiple_readers && cohort.len() < MAX_OBSERVATION_COHORT {
                    let deadline = Instant::now() + RELEASE_BATCH_WINDOW;
                    while cohort.len() < MAX_OBSERVATION_COHORT {
                        match tokio::time::timeout_at(deadline, receive.recv()).await {
                            Ok(Some(request)) => cohort.push(request),
                            _ => break,
                        }
                    }
                }
                release(&service, cohort).await;
            }
        }
    }
}

async fn release(service: &AgentObservationService, cohort: Vec<Fence>) {
    let ids: std::collections::BTreeSet<_> = cohort
        .iter()
        .map(|request| request.authorization.id.to_string())
        .collect();
    let ids: Vec<_> = ids.into_iter().map(AgentId).collect();
    let tokens: Vec<_> = {
        let registry = service.registry.lock().unwrap();
        ids.iter()
            .filter_map(|id| {
                registry
                    .lanes
                    .get(id.as_str())
                    .map(|lane| lane.authority.token.clone())
            })
            .collect()
    };
    let ids: Vec<_> = tokens.iter().map(|token| token.agent_id.clone()).collect();
    let checked = futures_util::future::join_all(
        cohort
            .iter()
            .map(|request| membership(service, &request.authorization)),
    )
    .await;
    let result = tokio::time::timeout(Duration::from_secs(10), service.store.fresh(&tokens)).await;
    let facts = match result {
        Ok(Ok(facts)) if facts.len() == ids.len() => ids
            .into_iter()
            .zip(facts)
            .map(|(id, fact)| (id.to_string(), fact))
            .collect::<BTreeMap<_, _>>(),
        _ => {
            for request in cohort {
                let _ = request.reply.send(Err(unavailable()));
            }
            return;
        }
    };
    for (request, membership) in cohort.into_iter().zip(checked) {
        if request.reply.is_closed() {
            continue;
        }
        let result = async {
            membership?;
            let admitted = service
                .registry
                .lock()
                .unwrap()
                .lanes
                .get(request.authorization.id.as_str())
                .is_some_and(|lane| {
                    lane.authority.current()
                        && request
                            .authorization
                            .observing
                            .and_then(|id| lane.subscribers.get(&id))
                            .is_some_and(|subscriber| !subscriber.terminal)
                });
            if !admitted {
                return Err(unavailable());
            }
            let snapshot = facts
                .get(request.authorization.id.as_str())
                .ok_or_else(unavailable)?
                .as_ref()
                .map_err(Clone::clone)?;
            service
                .reads
                .validate_observation_snapshot(&request.authorization, snapshot)
                .await?;
            if !service.store.ready() {
                return Err(unavailable());
            }
            let current = service
                .registry
                .lock()
                .unwrap()
                .lanes
                .get(request.authorization.id.as_str())
                .is_some_and(|lane| lane.authority.current());
            if !current {
                return Err(unavailable());
            }
            Ok(snapshot.clone())
        }
        .await;
        if let Err(error) = &result {
            if let Some(subscriber) = service
                .registry
                .lock()
                .unwrap()
                .lanes
                .get_mut(request.authorization.id.as_str())
                .and_then(|lane| {
                    request
                        .authorization
                        .observing
                        .and_then(|id| lane.subscribers.get_mut(&id))
                })
            {
                subscriber.terminal = true;
                subscriber.notice.send_replace(Some(Err(error.clone())));
            }
        }
        let _ = request.reply.send(result);
    }
}

async fn periodic(service: &AgentObservationService, hint: bool) {
    let hinted = if hint {
        std::mem::take(&mut *service.hinted.lock().unwrap())
    } else {
        Default::default()
    };
    let authorities: Vec<_> = service
        .registry
        .lock()
        .unwrap()
        .lanes
        .values()
        .filter(|lane| {
            lane.subscribers
                .values()
                .any(|subscriber| !subscriber.terminal)
                && (!hint || hinted.contains(lane.authority.token.agent_id.as_str()))
        })
        .map(|lane| lane.authority.clone())
        .collect();
    for cohort in authorities.chunks(MAX_OBSERVATION_COHORT) {
        let tokens: Vec<_> = cohort
            .iter()
            .map(|authority| authority.token.clone())
            .collect();
        let subscribers: Vec<_> = {
            let registry = service.registry.lock().unwrap();
            cohort
                .iter()
                .flat_map(|authority| {
                    registry
                        .lanes
                        .get(authority.token.agent_id.as_str())
                        .into_iter()
                        .flat_map(|lane| {
                            lane.subscribers
                                .iter()
                                .filter(|(_, subscriber)| !subscriber.terminal)
                                .map(|(id, subscriber)| (*id, subscriber.authorization.clone()))
                        })
                })
                .collect()
        };
        let mut memberships: BTreeMap<_, _> =
            futures_util::future::join_all(subscribers.iter().map(|(id, authorization)| async {
                (*id, membership(service, authorization).await)
            }))
            .await
            .into_iter()
            .collect();
        let started = Instant::now();
        let refreshed = tokio::time::timeout(Duration::from_secs(10), async {
            if hint {
                let facts = service.store.fresh(&tokens).await?;
                if facts.len() != cohort.len() {
                    return Err(unavailable());
                }
                Ok(cohort
                    .iter()
                    .zip(facts)
                    .map(|(authority, snapshot)| AgentObservationRefresh {
                        token: authority.token.clone(),
                        until: authority.deadline.borrow().unwrap_or(started),
                        snapshot: Some(snapshot),
                    })
                    .collect())
            } else {
                service.store.refresh(&tokens).await
            }
        })
        .await;
        let rows = match refreshed {
            Ok(Ok(rows)) if rows.len() == tokens.len() => rows,
            failure => {
                let reason = match failure {
                    Ok(Ok(_)) => "incomplete_cohort",
                    Ok(Err(_)) => "store_error",
                    Err(_) => "worker_deadline",
                };
                tracing::warn!(target: "cma.agent_observation", reason,
                    agents = cohort.len(), hint, "Revoking Agent observation cohort");
                for authority in cohort {
                    fail(service, authority);
                }
                continue;
            }
        };
        for (authority, row) in cohort.iter().zip(rows) {
            if row.token != authority.token
                || row.until <= started
                || authority.renew(row.until).is_err()
            {
                fail(service, authority);
                continue;
            }
            let Some(snapshot) = row.snapshot else {
                continue;
            };
            let subscribers: Vec<_> = service
                .registry
                .lock()
                .unwrap()
                .lanes
                .get(authority.token.agent_id.as_str())
                .map(|lane| {
                    lane.subscribers
                        .iter()
                        .filter(|(_, subscriber)| !subscriber.terminal)
                        .map(|(id, subscriber)| {
                            (
                                *id,
                                subscriber.authorization.clone(),
                                subscriber.notice.clone(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (id, authorization, notice) in subscribers {
                // A new subscriber may arrive while cohort IO is in flight.
                // Its opening/release has independent fresh authority; only
                // subscribers captured for this cohort receive its result.
                let Some(membership) = memberships.remove(&id) else {
                    continue;
                };
                let result = async {
                    membership?;
                    if !service.store.ready() || !authority.current() {
                        return Err(unavailable());
                    }
                    let snapshot = snapshot.as_ref().map_err(Clone::clone)?;
                    service
                        .reads
                        .validate_observation_snapshot(&authorization, snapshot)
                        .await?;
                    let agent = snapshot.lineage.agent.as_ref().ok_or_else(unavailable)?;
                    if agent.created_at != authority.token.created_at {
                        return Err(AgentReadError::NotFound);
                    }
                    Ok(AgentEventStreamHeader {
                        id: agent.id.clone(),
                        created_at: agent.created_at.clone(),
                        runtime_generation: agent.conversation.runtime_generation,
                        active_turn: agent.conversation.active_turn,
                        head: agent.conversation.last_seq,
                    })
                }
                .await;
                if result.is_err() {
                    if let Some(subscriber) = service
                        .registry
                        .lock()
                        .unwrap()
                        .lanes
                        .get_mut(authority.token.agent_id.as_str())
                        .and_then(|lane| lane.subscribers.get_mut(&id))
                    {
                        subscriber.terminal = true;
                    }
                }
                notice.send_replace(Some(result));
            }
        }
    }
}

fn fail(service: &AgentObservationService, authority: &AgentObservationAuthority) {
    authority.revoke();
    if let Some(lane) = service
        .registry
        .lock()
        .unwrap()
        .lanes
        .get(authority.token.agent_id.as_str())
    {
        for subscriber in lane.subscribers.values() {
            subscriber.notice.send_replace(Some(Err(unavailable())));
        }
    }
}

async fn membership(
    service: &AgentObservationService,
    authorization: &AgentEventStreamAuthorization,
) -> Result<(), AgentReadError> {
    tokio::time::timeout(
        Duration::from_secs(2),
        service.reads.validate_observation_membership(authorization),
    )
    .await
    .map_err(|_| unavailable())?
}
