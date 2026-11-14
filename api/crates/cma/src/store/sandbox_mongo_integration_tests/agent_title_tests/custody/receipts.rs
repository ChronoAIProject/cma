use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_retained_result_publishes_after_takeover_without_provider() {
    let f = Fixture::new("result_replay").await;
    let provider = Provider::new(false).await;
    let (claim, intent) = f.intent().await;
    let title = AgentTitle::new("Saved accepted result").unwrap();
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    assert!(f
        .a
        .accept_agent_title_effect(&claim, &intent, &title)
        .await
        .unwrap());
    let original = f.row().await.get("title_effect_custody").unwrap().clone();
    expire(&f.a, &f.id).await;
    append_kinds(
        &f.a,
        &f.id,
        completed_turn("Later request", "Later response", 8),
    )
    .await;
    let off = Arc::new(peer(&f.name, false).await);
    let service = AgentTitleService::new(off, "off-successor").unwrap();
    assert_eq!(
        service
            .generate_agent_title(&f.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(f.b.agent_title(&f.id).await.unwrap().unwrap().title, title);
    assert_eq!(f.row().await.get("title_effect_custody"), Some(&original));
    assert_eq!(provider.calls(), 0);
    assert!(f
        .a
        .accept_agent_title_effect(&claim, &intent, &title)
        .await
        .unwrap());
    assert!(!f
        .a
        .accept_agent_title_effect(&claim, &intent, &AgentTitle::new("Changed result").unwrap())
        .await
        .unwrap());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_user_override_and_expiry_preserve_unknown() {
    let f = Fixture::new("user_fence").await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    let user = AgentTitle::new("User wins").unwrap();
    f.b.set_user_agent_title(&f.id, &user, MINUTE.into())
        .await
        .unwrap();
    assert!(!f
        .a
        .accept_agent_title_effect(&claim, &intent, &AgentTitle::new("Late result").unwrap())
        .await
        .unwrap());
    assert!(!f
        .a
        .validate_agent_title_effect(&claim, &intent)
        .await
        .unwrap());
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    assert_eq!(f.b.agent_title(&f.id).await.unwrap().unwrap().title, user);
    f.cleanup().await;

    let f = Fixture::new("expired_result").await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    expire(&f.a, &f.id).await;
    assert!(!f
        .a
        .accept_agent_title_effect(&claim, &intent, &AgentTitle::new("Expired result").unwrap())
        .await
        .unwrap());
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn title_custody_lost_intent_and_result_ack_reconcile_exactly_once() {
    let f = Fixture::new("lost_acks").await;
    let provider = Provider::new(false).await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let app = format!("title-custody-{}", uuid::Uuid::new_v4());
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, &f.name)
            .await
            .unwrap()
            .with_title_effect_custody(true),
    );
    // Claim acquisition also updates, so acquire it before injecting an update
    // acknowledgement loss. Two exact operations must reconcile retained rows.
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(f.id.clone(), "lost-ack"),
            SystemTime::now(),
        )
        .await
        .unwrap()
        .unwrap();
    let transcript = store.agent_title_transcript(&f.id).await.unwrap().unwrap();
    let intent = AgentTitleEffectIntent::new(&claim, &transcript, &Probe).unwrap();
    lost_update_ack(&f.client, &app).await;
    let begun = store.begin_agent_title_effect(&claim, &intent).await;
    disable_failpoint(&f.client).await;
    assert!(begun.unwrap());
    assert!(store
        .validate_agent_title_effect(&claim, &intent)
        .await
        .unwrap());
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(
        provider.calls(),
        0,
        "rediscovery cannot reconstruct the originating capability"
    );
    let title = AgentTitle::new("Ack reconciled result").unwrap();
    lost_update_ack(&f.client, &app).await;
    let accepted = store
        .accept_agent_title_effect(&claim, &intent, &title)
        .await;
    disable_failpoint(&f.client).await;
    assert!(accepted.unwrap());
    enable_ambiguous_commits(&f.client, &app).await;
    let published = store
        .complete_agent_title(&claim, &title, AgentTitleSource::Generated, MINUTE.into())
        .await;
    disable_failpoint(&f.client).await;
    assert!(published.unwrap());
    assert!(store
        .complete_agent_title(&claim, &title, AgentTitleSource::Generated, MINUTE.into())
        .await
        .unwrap());
    assert_eq!(
        f.a.agent_log(&f.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TitleSet { .. }))
            .count(),
        1
    );
    f.cleanup().await;
}

struct Probe;
#[async_trait::async_trait]
impl PreparedAgentTitle for Probe {
    fn request_digest(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
    fn strategy(&self) -> &str {
        "ack-fixture-v1"
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        panic!("store-only proof")
    }
}
pub(super) async fn lost_update_ack(client: &Client, app: &str) {
    client.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
        "data": {"failCommands": ["update"], "appName": app,
            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "lost title custody acknowledgement"}}
    }).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_server_expiry_without_takeover_refuses_intent_and_result() {
    let f = Fixture::new("server_expiry").await;
    let request = AgentTitleClaimRequest::new(f.id.clone(), "expired-origin");
    let claim = AgentTitleClaim::acquired(
        request,
        AgentTitleClaimMode::Generate,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        SystemTime::UNIX_EPOCH + Duration::from_secs(46),
    )
    .with_attempts(1);
    let transcript = f.a.agent_title_transcript(&f.id).await.unwrap().unwrap();
    let intent = AgentTitleEffectIntent::new(&claim, &transcript, &Probe).unwrap();
    let mut fields = crate::store::agent_title_state::claim_fields(&claim, false);
    fields.extend(crate::store::agent_title_effect::initial_fields());
    f.a.agents
        .update_one(doc! {"_id": f.id.as_str()}, doc! {"$set": fields})
        .await
        .unwrap();
    assert!(!f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    assert!(matches!(
        f.b.agent_title_effect(&f.id).await.unwrap(),
        AgentTitleEffectState::Ready
    ));
    // Restore a pending intent created during this original lease, after its
    // deadline elapsed. No successor, user edit or token mismatch is involved.
    let effect = layer2_core::sandbox::AgentTitleEffect {
        version: 1,
        intent: intent.clone(),
        result: None,
    };
    let envelope = mongodb::bson::serialize_to_document(&effect).unwrap();
    f.a.agents
        .update_one(
            doc! {"_id": f.id.as_str()},
            doc! {"$set": {"title_effect_floor": 2_i32, "title_effect_custody": envelope}},
        )
        .await
        .unwrap();
    assert!(!f
        .a
        .validate_agent_title_effect(&claim, &intent)
        .await
        .unwrap());
    assert!(!f
        .a
        .accept_agent_title_effect(
            &claim,
            &intent,
            &AgentTitle::new("Expired original result").unwrap()
        )
        .await
        .unwrap());
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_claims_use_server_time_despite_caller_clock_jumps() {
    let f = Fixture::new("server_clock").await;
    let before = f
        .client
        .database("admin")
        .run_command(doc! {"hello": 1_i32})
        .await
        .unwrap()
        .get_datetime("localTime")
        .unwrap()
        .to_system_time();
    let request = AgentTitleClaimRequest::new(f.id.clone(), "skewed-origin");
    let claim =
        f.a.acquire_agent_title_claim(&request, SystemTime::UNIX_EPOCH)
            .await
            .unwrap()
            .unwrap();
    let after = f
        .client
        .database("admin")
        .run_command(doc! {"hello": 1_i32})
        .await
        .unwrap()
        .get_datetime("localTime")
        .unwrap()
        .to_system_time();
    assert!(claim.acquired_at() >= before && claim.acquired_at() <= after);
    assert_eq!(
        claim
            .expires_at()
            .duration_since(claim.acquired_at())
            .unwrap(),
        request.ttl()
    );
    assert!(f
        .b
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(f.id.clone(), "future-clock"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000)
        )
        .await
        .unwrap()
        .is_none());
    let off = peer(&f.name, false).await;
    assert!(off
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(f.id.clone(), "off-future-clock"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000)
        )
        .await
        .unwrap()
        .is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_user_title_wins_after_result_storage_before_publication() {
    let f = Fixture::new("user_after_result").await;
    let (claim, intent) = f.intent().await;
    let generated = AgentTitle::new("Accepted before user edit").unwrap();
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    assert!(f
        .a
        .accept_agent_title_effect(&claim, &intent, &generated)
        .await
        .unwrap());
    let stored = f.row().await.get("title_effect_custody").unwrap().clone();
    let user = AgentTitle::new("User chosen title").unwrap();
    f.b.set_user_agent_title(&f.id, &user, MINUTE.into())
        .await
        .unwrap();
    assert!(!f
        .a
        .complete_agent_title(
            &claim,
            &generated,
            AgentTitleSource::Generated,
            MINUTE.into()
        )
        .await
        .unwrap());
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(f.b.agent_title(&f.id).await.unwrap().unwrap().title, user);
    assert_eq!(f.row().await.get("title_effect_custody"), Some(&stored));
    f.cleanup().await;
}
