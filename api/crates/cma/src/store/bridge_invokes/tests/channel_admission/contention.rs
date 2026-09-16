//! Real transaction locks establish duplicate admission liveness and refusal.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::TryStreamExt;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client, ClientSession,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::Duration,
};

type Conflicts = Arc<Mutex<(BTreeMap<i32, String>, BTreeSet<String>)>>;

async fn monitored(c: &ChannelFixture) -> (Arc<MongoChannelCommandStore>, Conflicts) {
    let conflicts: Conflicts = Arc::new(Mutex::new((BTreeMap::new(), BTreeSet::new())));
    let observed = conflicts.clone();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut state = observed.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if matches!(event.command.get_bool("autocommit"), Ok(false)) =>
            {
                let identity = format!(
                    "{:?}:{:?}",
                    event.command.get("lsid"),
                    event.command.get("txnNumber")
                );
                state.0.insert(event.request_id, identity);
            }
            CommandEvent::Failed(event) => {
                if let Some(identity) = state.0.remove(&event.request_id) {
                    if crate::store::sandbox_mongo::is_transient(&event.failure) {
                        state.1.insert(identity);
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                state.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let key = crate::store::AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
    let bridges = Arc::new(
        MongoBridgeStore::open(&client, &c.f.database, "fixture-deployment", &key)
            .await
            .unwrap(),
    );
    let resources = Arc::new(
        MongoSandboxStore::open(&client, &c.f.database)
            .await
            .unwrap(),
    );
    let invokes = Arc::new(
        MongoBridgeInvokeStore::open(bridges, resources, &c.f.database)
            .await
            .unwrap(),
    );
    (Arc::new(MongoChannelCommandStore::new(invokes)), conflicts)
}

async fn hold_bridge(c: &ChannelFixture) -> ClientSession {
    let mut holder = c.f.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut holder)
        .await
        .unwrap();
    c.f.bridges
        .definitions
        .update_one(
            doc! {"_id":c.f.bridge.id.as_str()},
            doc! {"$inc":{"owned_contention_fixture":1_i64}},
        )
        .session(&mut holder)
        .await
        .unwrap();
    holder
}

async fn retained_logs(c: &ChannelFixture) -> Vec<Document> {
    c.f.resources
        .agent_log
        .find(doc! {})
        .sort(doc! {"_id":1_i32})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

async fn no_admission(c: &ChannelFixture, logs: &[Document]) {
    assert_eq!(retained_logs(c).await, logs);
    for name in [
        "agent_commands",
        "agent_command_orders",
        "channel_usage_events",
        "bridge_outbox_items",
        "bridge_outbox_invocations",
    ] {
        assert_eq!(
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{name}"
        );
    }
    let inbox =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id":&c.inbox.id})
            .await
            .unwrap()
            .unwrap();
    assert!(!inbox.contains_key("command_correlation"));
    assert!(!inbox.contains_key("command_admission_fence"));
}

async fn one_admission(
    c: &ChannelFixture,
    request: &ChannelCommandReservation,
    record: &AdmittedAgentCommand,
    logs: &[Document],
) {
    assert_eq!(retained_logs(c).await, logs);
    assert_eq!(record.receipt.agent_order, 1);
    assert_eq!(record.status, AgentResponseStatus::WaitingAction);
    assert_eq!(record.work, CommandWorkState::default());
    for (name, expected) in [
        ("agent_commands", 1),
        ("agent_command_orders", 1),
        ("channel_usage_events", 2),
        ("bridge_outbox_items", 0),
        ("bridge_outbox_invocations", 0),
    ] {
        assert_eq!(
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            expected,
            "{name}"
        );
    }
    let row =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id":record.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        row.get("channel_admission"),
        Some(&record_bson(&request.sidecar().unwrap()).unwrap())
    );
    let order =
        c.f.resources
            .agent_command_orders
            .find_one(doc! {"_id":c.f.target.agent_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(order.get_i64("last_order").unwrap(), 1);
    let inbox =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id":&c.inbox.id})
            .await
            .unwrap()
            .unwrap();
    let correlation = inbox.get_document("command_correlation").unwrap();
    assert_eq!(
        correlation.get_str("command_id").unwrap(),
        record.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        correlation.get_str("response_id").unwrap(),
        record.receipt.response_id.as_ref().unwrap().as_str()
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_admission_survives_six_real_write_conflicts_and_replays_once() {
    let c = ChannelFixture::new().await;
    let logs = retained_logs(&c).await;
    let (commands, conflicts) = monitored(&c).await;
    let request = c.request().await;
    let duplicate = c.request().await;
    assert_eq!(
        request.reservation().scope(),
        duplicate.reservation().scope()
    );
    assert_eq!(
        request.reservation().key_sha256,
        duplicate.reservation().key_sha256
    );
    assert_eq!(request.sidecar().unwrap(), duplicate.sidecar().unwrap());
    let mut holder = hold_bridge(&c).await;
    let contender = commands.clone();
    let original = request.clone();
    let task = tokio::spawn(async move { contender.reserve_channel_command(original).await });
    let observed = tokio::time::timeout(Duration::from_secs(4), async {
        while conflicts.lock().unwrap().1.len() < 6 && !task.is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    // Always release the real lock before making assertions. Old five-attempt
    // admission finishes before this release with Ledger(Conflict).
    holder.abort_transaction().await.unwrap();
    let attempts = conflicts.lock().unwrap().1.len();
    let result = tokio::time::timeout(Duration::from_secs(9), task)
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "causal channel contention: {attempts} distinct transient transactions, result={:?}",
        result.as_ref().map(|r| &r.receipt.envelope.command_id)
    );
    observed.unwrap();
    let record =
        result.expect("same retained Channel input must survive the held competing transaction");
    assert!(
        attempts >= 6,
        "observed only {attempts} distinct transaction conflicts"
    );
    let (a, b) = tokio::join!(
        commands.reserve_channel_command(duplicate),
        c.commands.reserve_channel_command(request.clone())
    );
    assert_eq!(a.unwrap(), record);
    assert_eq!(b.unwrap(), record);
    let (reopened, _) = monitored(&c).await;
    assert_eq!(
        reopened
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        record
    );
    one_admission(&c, &request, &record, &logs).await;
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn persistent_channel_contention_is_unavailable_and_same_key_recovers() {
    let c = ChannelFixture::new().await;
    let logs = retained_logs(&c).await;
    let (commands, conflicts) = monitored(&c).await;
    let request = c.request().await;
    let mut holder = hold_bridge(&c).await;
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        commands.reserve_channel_command(request.clone()),
    )
    .await;
    holder.abort_transaction().await.unwrap();
    let elapsed = started.elapsed();
    let result = result.unwrap();
    eprintln!("persistent channel contention: {} distinct transient transactions in {elapsed:?}, result={:?}", conflicts.lock().unwrap().1.len(), result.as_ref().map(|r| &r.receipt.envelope.command_id));
    assert!(matches!(
        result,
        Err(ChannelCommandAdmissionError::Unavailable)
    ));
    assert!(conflicts.lock().unwrap().1.len() >= 6);
    assert!(elapsed >= Duration::from_secs(4) && elapsed < Duration::from_secs(12));
    no_admission(&c, &logs).await;
    let record = commands
        .reserve_channel_command(request.clone())
        .await
        .unwrap();
    assert_eq!(
        commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        record
    );
    one_admission(&c, &request, &record, &logs).await;
    c.close().await;
}

struct ShortConnection {
    snapshot: ResourceAccessSnapshot,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for ShortConnection {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: vec![BridgeRouteObservation {
                    route_id: "route".into(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                }],
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 3,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: false,
                exact_profiles_available: false,
                existing_target: Some(self.snapshot.clone()),
                deadline: tokio::time::Instant::now() + Duration::from_secs(1),
            },
        )
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_contention_never_renews_original_authority_or_writes_after_expiry() {
    let c = ChannelFixture::new().await;
    let logs = retained_logs(&c).await;
    let (commands, conflicts) = monitored(&c).await;
    let connection = Arc::new(ShortConnection {
        snapshot: c.f.authority.snapshot.lock().unwrap().clone().unwrap(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let definitions = BridgeService::new(
        c.f.bridges.clone(),
        c.f.authority.clone(),
        connection.clone(),
    );
    let service = BridgeInvokeService::new(
        definitions,
        Arc::new(ResourceAccessService::new(
            c.f.resources.clone(),
            c.f.authority.clone(),
        )),
        c.f.resources.clone(),
        c.f.store.clone(),
    );
    let proof = service
        .authorize(c.f.request(
            &c.grant,
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: c.inbox.id.clone(),
            },
        ))
        .await
        .unwrap();
    let deadline = proof.deadline();
    let request =
        ChannelCommandReservation::prepare(proof, c.inbox.clone(), c.allocation.clone()).unwrap();
    let mut holder = hold_bridge(&c).await;
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        commands.reserve_channel_command(request.clone()),
    )
    .await;
    holder.abort_transaction().await.unwrap();
    let result = result.unwrap();
    assert!(matches!(
        result,
        Err(ChannelCommandAdmissionError::Invoke(
            BridgeInvokeError::Unavailable
        ))
    ));
    assert!(tokio::time::Instant::now() >= deadline);
    assert_eq!(request.proof().deadline(), deadline);
    assert_eq!(
        connection.calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let attempts = conflicts.lock().unwrap().1.len();
    assert!(attempts > 0);
    // Releasing contention does not make the expired proof usable. The same
    // request cannot start another write, and the awaited task has no late work.
    assert!(matches!(
        commands.reserve_channel_command(request.clone()).await,
        Err(ChannelCommandAdmissionError::Invoke(
            BridgeInvokeError::Unavailable
        ))
    ));
    assert_eq!(conflicts.lock().unwrap().1.len(), attempts);
    no_admission(&c, &logs).await;
    let fresh = c.request().await;
    assert_eq!(
        fresh.reservation().envelope.command_id,
        request.reservation().envelope.command_id
    );
    let record = commands.reserve_channel_command(fresh).await.unwrap();
    one_admission(&c, &request, &record, &logs).await;
    c.close().await;
}
