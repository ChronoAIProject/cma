use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use layer2_core::bridge::ingress::*;
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::*;
use crate::store::AuthEncryptionKey;
#[path = "inbox_control_intents_tests.rs"]
mod control_intents;
#[path = "inbox_controls.rs"]
mod controls;
#[path = "inbox_event_matrix.rs"]
mod event_matrix;
#[path = "inbox_receipts.rs"]
mod receipts;
#[path = "inbox_redelivery.rs"]
mod redelivery;
#[path = "inbox_reply_addressing.rs"]
mod reply_addressing;
#[path = "inbox_work.rs"]
mod work;

const KEY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../layer2-core/src/bridge/ingress/fixtures/callback-test-only.pk8"
));
const JWKS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../layer2-core/src/bridge/ingress/fixtures/jwks.json"
));

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn encryption_key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64(&STANDARD.encode([49_u8; 32])).unwrap()
}
fn sign(claims: Value) -> String {
    let pair = RsaKeyPair::from_pkcs8(KEY).unwrap();
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT","kid":"fixture-key"}"#),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let mut signature = vec![0; pair.public().modulus_len()];
    pair.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}
pub(super) fn callback(message: &str, text: &str) -> (Vec<u8>, String, String) {
    callback_for_sender(message, text, 12345)
}
pub(super) fn callback_for_sender(
    message: &str,
    text: &str,
    sender: u64,
) -> (Vec<u8>, String, String) {
    let reply = sign(
        json!({"iss":"https://nyx.example","aud":"channel-relay/reply","iat":now(),"exp":now()+1800,"jti":"reply-1","token_type":"relay_reply",
        "api_key_id":"key-1","conversation_id":"route-1","inbound_message_id":message,"platform":"telegram"}),
    );
    let body=serde_json::to_vec(&json!({"message_id":message,"correlation_id":"callback-1","platform":"telegram","reply_token":reply,
        "agent":{"api_key_id":"key-1","name":"Fixture"},"conversation":{"id":"route-1","platform_id":"-100123","type":"private"},
        "thread_id":"protected-interaction-secret","sender":{"platform_id":sender.to_string()},"content":{"type":"text","text":text},"timestamp":"2026-09-15T00:00:00Z",
        "raw_platform_data":{"secret":"excluded","update_id":99,"message":{"message_id":42,"date":1700000000,
            "chat":{"id":-100123,"type":"supergroup"},"from":{"id":sender,"is_bot":false},"text":text}}})).unwrap();
    let signed = sign(
        json!({"iss":"https://nyx.example","aud":"channel-relay/callback","iat":now(),"exp":now()+300,"jti":"callback-1","token_type":"relay_callback",
        "api_key_id":"key-1","message_id":message,"platform":"telegram","body_sha256":format!("{:x}",Sha256::digest(&body))}),
    );
    (body, signed, reply)
}
pub(super) async fn write(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
) -> InboxWrite {
    write_with_key_lease(store, id, body, token, Duration::from_secs(60)).await
}
async fn write_with_key_lease(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
    key_lease: Duration,
) -> InboxWrite {
    write_with_extractor(
        store,
        id,
        body,
        token,
        key_lease,
        &crate::bridge::ingress::NyxidCapabilities,
    )
    .await
}

async fn write_with_extractor(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
    key_lease: Duration,
    extractor: &dyn CallbackCapabilityExtractor,
) -> InboxWrite {
    write_using_validator(store, id, body, token, key_lease, extractor, &Authority).await
}

pub(super) async fn write_using_validator(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
    key_lease: Duration,
    extractor: &dyn CallbackCapabilityExtractor,
    validator: &dyn BridgeConnectionValidator,
) -> InboxWrite {
    let bridge = store.bridge(id).await.unwrap().unwrap();
    let proof = validator
        .validate_connection(&bridge.owner, &bridge.current().unwrap().config)
        .await
        .unwrap();
    let binding = CallbackBinding::new(bridge.clone(), proof).unwrap();
    let keys = CallbackKeySet::from_pinned_jwks(
        "https://nyx.example".into(),
        JWKS,
        tokio::time::Instant::now() + key_lease,
    )
    .unwrap();
    let verifier = CallbackVerifier::new(CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    })
    .unwrap();
    let payload: Value = serde_json::from_slice(body).unwrap();
    verifier
        .preauthenticate(
            bridge,
            &keys,
            CallbackHeaders {
                callback_token: token,
                user_token: None,
                message_id: payload["message_id"].as_str().unwrap(),
                platform: "telegram",
                timestamp: &layer2_core::now_rfc3339(),
            },
            body,
        )
        .unwrap()
        .classify_capabilities(body, extractor)
        .unwrap()
        .bind(binding)
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn uncertain_commit_with_expired_proof_stays_unknown_and_fresh_retry_recovers_receipt() {
    let (admin, database, _, _, id) = setup().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!(
        "cma-callback-reconciliation-{}",
        uuid::Uuid::new_v4().simple()
    );
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = open_store(&client, &database).await;
    let inbox =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let (body, token, _) = callback("uncertain-commit", "retained message");
    // Each commit takes 300 ms and reports an ambiguous write-concern result.
    // The transaction commits, but its original key proof expires before the
    // bounded commit helper returns. Keep the failpoint active across driver
    // retries too, so a sixth wire attempt cannot silently recover success.
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "alwaysOn",
            "data": {
                "failCommands": ["commitTransaction"], "appName": &app_name,
                "blockConnection": true, "blockTimeMS": 300_i32,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous callback commit after proof expiry"}
            }
        })
        .await
        .unwrap();
    let write = write_with_key_lease(&store, &id, &body, &token, Duration::from_secs(1)).await;
    let expected = write.record().receipt();
    let outcome = inbox
        .accept(write)
        .await
        .map(|value| value.into_message().unwrap());
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .expect("disable failpoint before assertions");
    assert_eq!(outcome, Err(CallbackError::Unavailable));
    let rows = crate::store::durability::majority_collection::<Document>(
        &client.database(&database),
        "bridge_callback_inbox",
    );
    let document = rows
        .find_one(doc! {"_id": &expected.inbox_id})
        .await
        .unwrap()
        .unwrap();
    let retained: StoredInbox = crate::store::sandbox_mongo::read_record(&document).unwrap();
    assert_eq!(
        retained.receipt(),
        expected,
        "unknown response must not discard the durable result"
    );
    let fresh = write_with_key_lease(&store, &id, &body, &token, Duration::from_secs(60)).await;
    assert_eq!(
        inbox
            .accept(fresh)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        expected
    );
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn uncertain_commit_reconciliation_preserves_observed_conflict_and_exact_original_receipt() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("reconcile", "original message");
    let original = inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let duplicate = write(&store, &id, &body, &token).await;
    assert_eq!(
        inbox
            .reconcile_commit(&duplicate)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        original
    );
    let (changed, changed_token, _) = callback("reconcile", "substituted message");
    let conflict = write(&store, &id, &changed, &changed_token).await;
    assert_eq!(
        inbox
            .reconcile_commit(&conflict)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::ReplayConflict)
    );
    let (missing, missing_token, _) = callback("unobserved", "unobserved message");
    let missing = write(&store, &id, &missing, &missing_token).await;
    assert_eq!(
        inbox
            .reconcile_commit(&missing)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    client.database(&database).drop().await.unwrap();
}
async fn setup() -> (
    Client,
    String,
    Arc<MongoBridgeStore>,
    MongoCallbackInbox,
    BridgeId,
) {
    setup_with_config(config()).await
}

async fn setup_with_config(
    config: BridgeConfig,
) -> (
    Client,
    String,
    Arc<MongoBridgeStore>,
    MongoCallbackInbox,
    BridgeId,
) {
    let (client, database, store) = fixture().await;
    let service = service(store.clone());
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    service
        .mutate(
            &caller(),
            None,
            change(
                &id,
                0,
                "create",
                BridgeChange::Create {
                    owner: owner(),
                    config: Box::new(config),
                },
            ),
        )
        .await
        .unwrap();
    admit_custody(&service, &store, &id, 1).await;
    service
        .mutate(
            &caller(),
            None,
            change(&id, 1, "enable", BridgeChange::Enable),
        )
        .await
        .unwrap();
    let inbox =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    (client, database, store, inbox, id)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn local_custody_revocation_rejects_previously_verified_new_work_and_duplicate_receipts() {
    use crate::bridge::custody::BridgeCredentialStore;
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("accepted", "first");
    inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let duplicate = write(&store, &id, &body, &token).await;
    let (fresh_body, fresh_token, _) = callback("fresh", "second");
    let fresh = write(&store, &id, &fresh_body, &fresh_token).await;
    let management = service(store.clone())
        .management_lease(&caller(), None, &id, 2)
        .await
        .unwrap();
    let reference = management
        .record()
        .current()
        .unwrap()
        .config
        .connector
        .custody
        .clone();
    store
        .credentials
        .revoke(&management, &reference, 1, &"f".repeat(64))
        .await
        .unwrap();
    assert_eq!(
        inbox
            .accept(fresh)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::BindingUnavailable)
    );
    assert_eq!(
        inbox
            .accept(duplicate)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::BindingUnavailable)
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_duplicate_race_restart_and_encrypted_reply_expiry_preserve_one_acceptance() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, reply) = callback("message-1", "sensitive body");
    let a = write(&store, &id, &body, &token).await;
    let b = write(&store, &id, &body, &token).await;
    let (a, b) = tokio::join!(inbox.accept(a), inbox.accept(b));
    let receipt = a.unwrap().into_message().unwrap();
    assert_eq!(b.unwrap().into_message().unwrap(), receipt);
    assert_eq!(
        receipt.execution_hold,
        Some(InboxExecutionHold::WorkerUnavailable)
    );
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    assert_eq!(
        reopened
            .accept(write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap(),
        receipt
    );
    let database_handle = client.database(&database);
    let documents = database_handle.collection::<Document>("bridge_callback_inbox");
    assert_eq!(documents.count_documents(doc! {}).await.unwrap(), 1);
    let raw = documents.find_one(doc! {}).await.unwrap().unwrap();
    let retained: StoredInbox = crate::store::sandbox_mongo::read_record(&raw).unwrap();
    assert_eq!(
        retained.execution_hold,
        Some(InboxExecutionHold::WorkerUnavailable)
    );
    assert_eq!(retained.envelope.conversation_type, "group");
    assert!(retained.capabilities.is_some());
    assert!(
        layer2_core::bridge::partition::NormalizedPartitionSource::from_stored(
            &retained,
            &store.bridge(&id).await.unwrap().unwrap(),
        )
        .is_ok()
    );
    let encoded = serde_json::to_string(&raw).unwrap();
    assert!(!encoded.contains(&reply));
    assert!(!encoded.contains("excluded"));
    assert!(!encoded.contains("protected-interaction-secret"));
    let encrypted = database_handle
        .collection::<Document>("bridge_callback_replies")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!serde_json::to_string(&encrypted).unwrap().contains(&reply));
    assert!(!serde_json::to_string(&encrypted)
        .unwrap()
        .contains("protected-interaction-secret"));
    assert!(encrypted.contains_key("thread_context"));
    assert_eq!(
        reopened
            .reply_context(&receipt.inbox_id)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        reply
    );

    let other_deployment =
        MongoCallbackInbox::open(&store, &database, "wrong-deployment", &encryption_key())
            .await
            .unwrap();
    assert!(other_deployment
        .reply_context(&receipt.inbox_id)
        .await
        .is_err());
    database_handle
        .collection::<Document>("bridge_callback_replies")
        .update_one(
            doc! {"_id":&receipt.inbox_id},
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::from_millis((now()-1)*1000)}},
        )
        .await
        .unwrap();
    assert!(reopened
        .reply_context(&receipt.inbox_id)
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn authenticated_body_conflict_and_disabled_binding_cannot_add_work() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("message-1", "first");
    inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    let (changed, changed_token, _) = callback("message-1", "changed");
    assert_eq!(
        inbox
            .accept(write(&store, &id, &changed, &changed_token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::ReplayConflict)
    );
    let (fresh, fresh_token, _) = callback("message-2", "fresh");
    let admitted = write(&store, &id, &fresh, &fresh_token).await;
    service(store)
        .mutate(
            &caller(),
            None,
            change(
                &id,
                2,
                "disable",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        inbox
            .accept(admitted)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::BindingUnavailable)
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn database_failure_rolls_back_inbox_reply_and_rate_before_acknowledgement() {
    let (client, database, store, inbox, id) = setup().await;
    // Fault injection touches only this test's randomly named disposable DB.
    client.database(&database).run_command(doc!{"collMod":"bridge_callback_inbox","validator":{"never_present":{"$exists":true}},"validationLevel":"strict"}).await.unwrap();
    let (body, token, _) = callback("message-1", "first");
    assert_eq!(
        inbox
            .accept(write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Unavailable)
    );
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_replies",
        "bridge_callback_rates",
    ] {
        assert_eq!(
            client
                .database(&database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{name}"
        );
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn configured_queue_capacity_and_original_receipts_are_enforced_transactionally() {
    let (client, database, store, inbox, id) = setup().await;
    for number in 0..5 {
        let (body, token, _) = callback(&format!("message-{number}"), "queued");
        inbox
            .accept(write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap())
            .unwrap();
    }
    let (body, token, _) = callback("message-extra", "full");
    assert_eq!(
        inbox
            .accept(write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Capacity)
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        5
    );
    client.database(&database).drop().await.unwrap();
}
