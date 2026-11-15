//! Real, authenticated Mongo clients share only persisted admission authority.
#[allow(dead_code)]
#[path = "support/database.rs"]
mod database;
#[path = "support/replica_delivery_mongo.rs"]
mod delivery;
#[path = "support/replica_delivery.rs"]
mod replica_delivery;
#[path = "support/replica_execution.rs"]
mod replica_execution;
#[allow(dead_code)]
#[path = "relay/signer.rs"]
mod signer;
#[allow(dead_code)]
#[path = "relay/support.rs"]
mod support;

use cmaeg::ingress::{
    launch::LaunchRequest,
    routing_plan::{plan, OrdinaryPlan, RoutingOptions},
    routing_store::{AdmissionError, AdmissionQueue, AdmittedPlan, Candidate},
};
use futures_util::future::{join_all, BoxFuture};
use replica_delivery::{routed_channel, runner_keys, DeliveryOracle};
use std::{collections::BTreeSet, sync::Arc};
use support::{Envelope, Fixture};

async fn verified(fixture: &Fixture, text: &str) -> (LaunchRequest, Vec<u8>) {
    let mut envelope = Envelope::new();
    envelope.payload["content"]["text"] = text.into();
    envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
    let (body, token) = envelope.signed();
    let response = fixture.request(body.clone(), &token).send().await.unwrap();
    assert_eq!(response.status(), 202);
    let request = fixture
        .recorder
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    (request, body)
}

fn candidate(request: &LaunchRequest, body: &[u8]) -> Candidate {
    let now = support::now();
    let OrdinaryPlan::Routed(routed) = plan(
        request,
        &runner_keys(),
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &BTreeSet::from(["cma_codex".into()]),
        },
    )
    .unwrap() else {
        panic!("routed fixture required")
    };
    Candidate::prepare(request, &routed, body, &runner_keys(), now).unwrap()
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn independent_clients_dedupe_verified_webhooks_and_conflict_on_changed_body() {
    let fixture = Fixture::new(routed_channel, Default::default()).await;
    fixture
        .recorder
        .outcome
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let (request, body) = verified(&fixture, "original").await;
    let original = Arc::new(candidate(&request, &body));
    let (changed_request, changed_body) = verified(&fixture, "changed").await;
    let changed = candidate(&changed_request, &changed_body);
    assert_eq!(original.identity(), changed.identity());
    assert_ne!(original.request_hash(), changed.request_hash());

    for count in [2, 3, 5] {
        let database =
            database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
        let clients = join_all(
            (0..count).map(|_| DeliveryOracle::open(&database.uri, &database.name, runner_keys())),
        )
        .await;
        let barrier = Arc::new(tokio::sync::Barrier::new(count));
        let outcomes = join_all(clients.iter().map(|client| {
            let barrier = barrier.clone();
            let original = original.clone();
            async move {
                barrier.wait().await;
                client.plans.admit(&original).await.unwrap()
            }
        }))
        .await;
        let retained = &outcomes[0];
        assert_eq!(outcomes.len(), count);
        assert!(outcomes.iter().all(|plan| plan == retained));
        assert_eq!(retained.source_body, body);
        assert_eq!(retained.targets.len(), 1);
        for client in &clients {
            assert_eq!(
                client.plans.admit(&changed).await,
                Err(AdmissionError::Conflict)
            );
            assert_eq!(
                client.plans.replay(&changed_request, &changed_body).await,
                Err(AdmissionError::Conflict)
            );
            assert_eq!(
                client.plan(original.identity()).await.as_ref(),
                Some(retained)
            );
        }
        drop(clients);

        // No client-local cache or dedupe table survives this replacement.
        let restarted = DeliveryOracle::open(&database.uri, &database.name, runner_keys()).await;
        assert_eq!(
            restarted
                .plans
                .replay(&request, &body)
                .await
                .unwrap()
                .as_ref(),
            Some(retained)
        );
        let mut cursor = None;
        let mut recovered = Vec::new();
        loop {
            let page = restarted
                .plans
                .recover_all_page(cursor.as_deref(), 32)
                .await
                .unwrap();
            assert!(page.issues.is_empty());
            recovered.extend(page.plans);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(recovered, vec![retained.clone()]);
        // Admission never fabricates a provider or delivery receipt.
        assert!(restarted
            .targets(retained)
            .await
            .values()
            .all(Option::is_none));
        database
            .admin
            .database(&database.name)
            .drop()
            .await
            .unwrap();
    }
}

struct UnavailableQueue;
impl AdmissionQueue for UnavailableQueue {
    fn enqueue<'a>(&'a self, _plan: &'a AdmittedPlan) -> BoxFuture<'a, Result<(), AdmissionError>> {
        Box::pin(async { Err(AdmissionError::Unavailable) })
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn queue_failure_retains_mongo_root_but_never_reports_accepted_or_recoverable_work() {
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let fixture = Fixture::new(routed_channel, Default::default()).await;
    fixture
        .recorder
        .outcome
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let (request, body) = verified(&fixture, "original").await;
    let original = candidate(&request, &body);
    let writer = DeliveryOracle::open(&database.uri, &database.name, runner_keys()).await;
    let plans = writer
        .plans
        .clone()
        .with_admission_queue(Arc::new(UnavailableQueue));
    assert_eq!(
        plans.admit(&original).await,
        Err(AdmissionError::Unavailable)
    );
    drop(plans);
    drop(writer);
    let restarted = DeliveryOracle::open(&database.uri, &database.name, runner_keys()).await;
    let retained = restarted.plan(original.identity()).await.unwrap();
    assert_eq!(retained.source_body, body);
    let plans = restarted
        .plans
        .clone()
        .with_admission_queue(Arc::new(UnavailableQueue));
    assert_eq!(
        plans.replay(&request, &body).await,
        Err(AdmissionError::Unavailable)
    );
    let page = plans.recover_all_page(None, 32).await.unwrap();
    assert!(page.plans.is_empty());
    assert_eq!(page.issues.len(), 1);
    assert_eq!(page.issues[0].record_id, original.admission_id());
    assert_eq!(page.issues[0].error, AdmissionError::Unavailable);
    assert!(restarted
        .targets(&retained)
        .await
        .values()
        .all(Option::is_none));
    database
        .admin
        .database(&database.name)
        .drop()
        .await
        .unwrap();
}
