use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::bridge::*;
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::{profiles::*, SubjectId};

use super::*;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn policy() -> CallbackTokenPolicy {
    CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    }
}
fn verifier() -> CallbackVerifier {
    CallbackVerifier::new(policy()).unwrap()
}
fn keys() -> CallbackKeySet {
    CallbackKeySet::from_pinned_jwks(
        policy().issuer,
        include_bytes!("fixtures/jwks.json"),
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn sign(claims: &Value) -> String {
    sign_header(
        json!({"alg":"RS256","typ":"JWT","kid":"fixture-key"}),
        claims,
    )
}
fn sign_header(header: Value, claims: &Value) -> String {
    let fixture: &[u8] = if header["kid"] == "rotated-key" {
        include_bytes!("fixtures/rotation-test-only.pk8")
    } else {
        include_bytes!("fixtures/callback-test-only.pk8")
    };
    let key = RsaKeyPair::from_pkcs8(fixture).unwrap();
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
    );
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}
fn binding(org: bool) -> CallbackBinding {
    binding_with_admission(org, BridgeAdmissionPolicy::All)
}

fn binding_with_admission(org: bool, admission: BridgeAdmissionPolicy) -> CallbackBinding {
    let owner = if org {
        ResourceOwner::Organization {
            subject: SubjectId::new("owner"),
        }
    } else {
        ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        }
    };
    let actor = ResourceActor::Human {
        subject: SubjectId::new("owner"),
    };
    let config = BridgeConfig {
        display_name: "Fixture".into(),
        connector: BridgeConnector {
            issuer: policy().issuer,
            base_url: policy().relay_audience,
            bot_id: "bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["nyx-route".into()],
            key_id: "selected-key".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/callback".into(),
        },
        partition: BridgePartitionPolicy::Conversation,
        placement: BridgePlacementPolicy::NewSandbox {
            sandbox_profile: SandboxProfileRef::new(crate::SandboxProfileId("sbp-1".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(
                crate::WorkspaceProfileId("wkp-1".into()),
                1,
            ),
            agent_profile: AgentProfileRef::new(crate::AgentProfileId("agp-1".into()), 1),
        },
        admission,
        reply: BridgeReplyPolicy::FinalResponse,
        limits: BridgeLimits {
            max_partitions: 5,
            max_sandboxes: 5,
            max_queued_messages: 10,
            max_messages_per_minute: 20,
            max_payload_bytes: 1048576,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 1200,
        },
    };
    let proof = BridgeValidation::from_observation(
        &owner,
        &config,
        BridgeConnectionObservation {
            connector: config.connector.clone(),
            bot_owner: owner.clone(),
            key_owner: owner.clone(),
            custody_owner: owner.clone(),
            routes: vec![BridgeRouteObservation {
                route_id: "nyx-route".into(),
                owner: owner.clone(),
                bot_id: "bot".into(),
                key_id: "selected-key".into(),
                enabled: true,
            }],
            key_active: true,
            custody_available: true,
            custody_revision: 1,
            key_state_version: 3,
            callback_conflict: false,
            renewable_sandbox_authority: true,
            exact_profiles_available: true,
            existing_target: None,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
        },
    )
    .unwrap();
    let time = "2026-09-15T00:00:00Z".to_string();
    CallbackBinding::new(
        BridgeRecord {
            id: BridgeId("brg_fixture".into()),
            owner,
            revision: 2,
            config_revision: 1,
            state: BridgeState::Enabled,
            emergency: Default::default(),
            generations: vec![BridgeGeneration {
                config_revision: 1,
                config,
                created_at: time.clone(),
                actor: actor.clone(),
                previous_work: None,
            }],
            created_at: time.clone(),
            created_by: actor,
            updated_at: time,
        },
        proof,
    )
    .unwrap()
}
fn payload() -> Value {
    json!({"message_id":"message-1","correlation_id":"callback-1","platform":"telegram", "agent":{"api_key_id":"selected-key","name":"Fixture"},
        "conversation":{"id":"nyx-route","platform_id":"actual-chat","type":"group"}, "sender":{"platform_id":"sender","display_name":"Fixture Sender"},
        "thread_id":"protected-interaction-secret","content":{"type":"text","text":"secret-message-body"},"timestamp":"2026-09-15T00:00:00Z","raw_platform_data":{"secret":"excluded-raw"}})
}
fn claims(body: &[u8]) -> Value {
    json!({"iss":policy().issuer,"aud":"channel-relay/callback","iat":now(),"exp":now()+300,"jti":"callback-1","token_type":"relay_callback",
        "api_key_id":"selected-key","message_id":"message-1","platform":"telegram","body_sha256":format!("{:x}",Sha256::digest(body))})
}
fn timestamp() -> String {
    crate::now_rfc3339()
}
fn verify_bytes(
    binding: CallbackBinding,
    body: &[u8],
    token: &str,
    user: Option<&str>,
) -> Result<InboxWrite, CallbackError> {
    verifier().verify(
        binding,
        &keys(),
        CallbackHeaders {
            callback_token: token,
            user_token: user,
            message_id: "message-1",
            platform: "telegram",
            timestamp: &timestamp(),
        },
        body,
    )
}
fn accept(payload: Value) -> Result<InboxWrite, CallbackError> {
    let body = serde_json::to_vec(&payload).unwrap();
    verify_bytes(binding(false), &body, &sign(&claims(&body)), None)
}

#[test]
fn valid_person_and_org_signatures_derive_owner_bot_and_distinct_chat_route() {
    let body = serde_json::to_vec(&payload()).unwrap();
    for organization in [false, true] {
        let b = binding(organization);
        let expected_owner = b.bridge.owner.clone();
        let write = verify_bytes(b, &body, &sign(&claims(&body)), None).unwrap();
        assert_eq!(write.record().owner, expected_owner);
        assert_eq!(write.record().bot_id, "bot");
        assert_eq!(write.record().envelope.route_id, "nyx-route");
        assert_eq!(write.record().envelope.chat_id, "actual-chat");
        assert_eq!(write.record().relay, RelayCorroboration::AbsentInboxOnly);
        assert!(!serde_json::to_string(write.record())
            .unwrap()
            .contains("excluded-raw"));
        assert_eq!(write.thread_context(), Some("protected-interaction-secret"));
        assert!(!serde_json::to_string(write.record())
            .unwrap()
            .contains("protected-interaction-secret"));
        let diagnostics = format!("{:?}", write.record().receipt());
        assert!(!diagnostics.contains("secret-message-body"));
    }
}

#[test]
fn exact_bytes_signature_algorithm_key_and_claim_mutations_fail_closed() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let token = sign(&claims(&body));
    let mut changed = body.clone();
    changed.push(b' ');
    assert!(matches!(
        verify_bytes(binding(false), &changed, &token, None),
        Err(CallbackError::Authentication)
    ));
    for (field, bad) in [
        ("iss", json!("https://foreign.example")),
        ("aud", json!("cma")),
        ("api_key_id", json!("foreign-key")),
        ("platform", json!("discord")),
        ("message_id", json!("foreign-message")),
        ("token_type", json!("access")),
        ("exp", json!(now() - 61)),
        // Signing and earlier cases can cross a wall-clock second. Exercise
        // the exact 60/61-second boundary separately with a fixed clock below.
        ("iat", json!(now() + 120)),
        ("nbf", json!(now() + 120)),
        ("jti", json!("foreign-correlation")),
    ] {
        let mut c = claims(&body);
        c[field] = bad;
        assert!(
            verify_bytes(binding(false), &body, &sign(&c), None).is_err(),
            "{field}"
        );
    }
    for header in [
        json!({"alg":"HS256","typ":"JWT","kid":"fixture-key"}),
        json!({"alg":"none","typ":"JWT","kid":"fixture-key"}),
        json!({"alg":"RS256","typ":"JWT","kid":"missing"}),
        json!({"alg":"RS256","typ":"JWT","kid":"fixture-key","jku":"https://foreign.example/keys"}),
    ] {
        assert!(verify_bytes(
            binding(false),
            &body,
            &sign_header(header, &claims(&body)),
            None
        )
        .is_err());
    }
    let mut forged = token.into_bytes();
    let last = forged.len() - 5;
    forged[last] = if forged[last] == b'A' { b'B' } else { b'A' };
    assert!(verify_bytes(
        binding(false),
        &body,
        std::str::from_utf8(&forged).unwrap(),
        None
    )
    .is_err());
}

#[test]
fn signed_claim_clock_skew_boundaries_use_the_original_observation() {
    let observed_at = 1_800_000_000;
    for (field, value, accepted) in [
        ("iat", observed_at + 60, true),
        ("iat", observed_at + 61, false),
        ("nbf", observed_at + 60, true),
        ("nbf", observed_at + 61, false),
        ("exp", observed_at - 59, true),
        ("exp", observed_at - 60, false),
    ] {
        let mut raw = json!({"iss":policy().issuer,"aud":"channel-relay/callback",
            "iat":observed_at-120,"exp":observed_at+180,"jti":"fixed-clock",
            "token_type":"relay_callback"});
        raw[field] = json!(value);
        let claims: super::jwt::StandardClaims = serde_json::from_value(raw).unwrap();
        assert_eq!(
            claims
                .require(
                    &policy(),
                    "channel-relay/callback",
                    "relay_callback",
                    300,
                    observed_at
                )
                .is_ok(),
            accepted,
            "{field}={value}"
        );
    }
}

#[test]
fn signed_payload_cannot_select_another_route_key_or_untrusted_resource() {
    for (pointer, value) in [
        ("/conversation/id", json!("actual-chat")),
        ("/agent/api_key_id", json!("foreign-key")),
        ("/correlation_id", json!("wrong")),
    ] {
        let mut p = payload();
        *p.pointer_mut(pointer).unwrap() = value;
        assert!(accept(p).is_err());
    }
    let mut p = payload();
    p["owner"] = json!("foreign-owner");
    assert!(matches!(accept(p), Err(CallbackError::Payload)));
    let mut p = payload();
    p["bot_id"] = json!("foreign-bot");
    assert!(accept(p).is_err());
    let mut b = binding(false);
    b.bridge.emergency.ingress_stopped = true;
    let body = serde_json::to_vec(&payload()).unwrap();
    assert!(verify_bytes(b, &body, &sign(&claims(&body)), None).is_err());
}

#[test]
fn bounded_fields_and_attachments_fail_before_persistence() {
    let mut p = payload();
    p["content"]["attachments"] = json!((0..33)
        .map(|_| json!({"content_type":"image","url":""}))
        .collect::<Vec<_>>());
    assert!(matches!(accept(p), Err(CallbackError::Bounds)));
    let mut p = payload();
    p["sender"]["platform_id"] = json!("s".repeat(257));
    assert!(matches!(accept(p), Err(CallbackError::Bounds)));
    let mut p = payload();
    p["content"]["text"] = json!("a".repeat(1048576));
    assert!(matches!(accept(p), Err(CallbackError::Bounds)));
}

#[test]
fn denied_sender_is_durably_auditable_without_retained_reply_or_thread_credentials() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let binding = binding_with_admission(
        false,
        BridgeAdmissionPolicy::Allowlist {
            sender_ids: vec!["different-sender".into()],
        },
    );
    let write = verify_bytes(binding, &body, &sign(&claims(&body)), None).unwrap();
    assert_eq!(write.record().admission, InboxAdmission::SenderDenied);
    assert!(write.reply_token().is_none());
    assert!(write.thread_context().is_none());
    assert!(write.record().reply_expires_at.is_none());
    assert!(!write.record().envelope.protected_thread_context_present);
}

#[test]
fn pinned_nyxid_fractional_and_offset_timestamps_accept_equivalent_signed_instants() {
    // Pinned channel_relay_service.rs uses chrono::DateTime::to_rfc3339()
    // for both header time (line365) and event time (line651).
    for (shift, suffix, event) in [
        (
            0_i64,
            ".123456789+00:00",
            "2026-09-15T00:00:00.123456789+00:00",
        ),
        (8 * 3600, ".123+08:00", "2026-09-15T08:00:00.123+08:00"),
        (-5 * 3600, "-05:00", "2026-09-14T19:00:00-05:00"),
        (0, ".123456Z", "2026-09-15T00:00:00.123456Z"),
    ] {
        let current = now();
        let local = crate::rfc3339_at(UNIX_EPOCH + Duration::from_secs((current + shift) as u64));
        let header = format!("{}{suffix}", local.trim_end_matches('Z'));
        assert_eq!(crate::bridge::provider_timestamp(&header), Some(current));
        let mut p = payload();
        p["timestamp"] = json!(event);
        let body = serde_json::to_vec(&p).unwrap();
        let token = sign(&claims(&body));
        let write = verifier()
            .verify(
                binding(false),
                &keys(),
                CallbackHeaders {
                    callback_token: &token,
                    user_token: None,
                    message_id: "message-1",
                    platform: "telegram",
                    timestamp: &header,
                },
                &body,
            )
            .unwrap();
        assert_eq!(write.record().envelope.timestamp, event);
    }
}

#[test]
fn malformed_provider_dates_and_offsets_fail_in_both_header_and_signed_payload() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let token = sign(&claims(&body));
    for malformed in [
        "2026-02-30T00:00:00+00:00",
        "2026-09-15T24:00:00+00:00",
        "2026-09-15T00:00:00.+00:00",
        "2026-09-15T00:00:00.123",
        "2026-09-15T00:00:00+25:00",
        "2026-09-15T00:00:00+00:60",
        "2026-09-15T00:00:00Ztrailing",
        "1969-12-31T23:59:59Z",
    ] {
        assert!(
            crate::bridge::provider_timestamp(malformed).is_none(),
            "{malformed}"
        );
        assert!(
            verifier()
                .verify(
                    binding(false),
                    &keys(),
                    CallbackHeaders {
                        callback_token: &token,
                        user_token: None,
                        message_id: "message-1",
                        platform: "telegram",
                        timestamp: malformed,
                    },
                    &body
                )
                .is_err(),
            "header {malformed}"
        );
        let mut p = payload();
        p["timestamp"] = json!(malformed);
        assert!(accept(p).is_err(), "payload {malformed}");
    }
}

#[test]
fn relay_header_is_separately_verified_and_confers_only_inbox_corroboration() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let callback = sign(&claims(&body));
    let c = json!({"iss":policy().issuer,"aud":policy().relay_audience,"iat":now(),"exp":now()+300,"jti":"relay-1","token_type":"access",
        "sub":"owner","relay":true,"relay_api_key_id":"selected-key","relay_allowed_service_ids":["service-1"],"relay_allowed_node_ids":[],
        "relay_allow_all_services":false,"relay_allow_all_nodes":false});
    let user = sign(&c);
    let write = verify_bytes(binding(false), &body, &callback, Some(&user)).unwrap();
    assert!(matches!(
        write.record().relay,
        RelayCorroboration::VerifiedInboxOnly { .. }
    ));
    assert!(!serde_json::to_string(write.record())
        .unwrap()
        .contains(&user));
    for (field, value) in [
        ("sub", json!("foreign-owner")),
        ("relay_api_key_id", json!("foreign-key")),
        ("relay", json!(false)),
        ("aud", json!("cma")),
        ("sa", json!(true)),
        ("delegated", json!(true)),
        ("cnf", json!({"jkt":"fake"})),
    ] {
        let mut invalid = c.clone();
        invalid[field] = value;
        assert!(
            verify_bytes(binding(false), &body, &callback, Some(&sign(&invalid))).is_err(),
            "{field}"
        );
    }
    assert!(verify_bytes(
        binding(false),
        &body,
        &callback,
        Some("unsigned-owner-claims")
    )
    .is_err());
}

#[test]
fn reply_context_has_verified_exact_scope_and_never_enters_normalized_record() {
    let reply_claims = json!({"iss":policy().issuer,"aud":"channel-relay/reply","iat":now(),"exp":now()+1800,"jti":"reply-1","token_type":"relay_reply",
        "api_key_id":"selected-key","conversation_id":"nyx-route","inbound_message_id":"message-1","platform":"telegram"});
    let reply = sign(&reply_claims);
    let mut p = payload();
    p["reply_token"] = json!(reply);
    let write = accept(p).unwrap();
    assert_eq!(write.reply_token(), Some(reply.as_str()));
    assert!(write.record().reply_expires_at.unwrap() <= now() + 1200);
    assert!(!serde_json::to_string(write.record())
        .unwrap()
        .contains(&reply));
    let mut wrong = reply_claims;
    wrong["conversation_id"] = json!("foreign-route");
    let mut p = payload();
    p["reply_token"] = json!(sign(&wrong));
    assert!(accept(p).is_err());
}

#[tokio::test(start_paused = true)]
async fn expired_keys_and_write_proofs_fail_closed_without_extending_authority() {
    let keyset = keys();
    let write = accept(payload()).unwrap();
    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(write.require_current(write.bridge()).is_err());
    tokio::time::advance(Duration::from_secs(55)).await;
    let result: Result<(Value, String), _> = keyset.verify(&sign(&json!({})), &policy().issuer);
    assert!(matches!(result, Err(CallbackError::KeysUnavailable)));
}

#[tokio::test(start_paused = true)]
async fn signature_precheck_cannot_gain_a_new_deadline_from_later_live_binding() {
    let original = binding(false);
    let body = serde_json::to_vec(&payload()).unwrap();
    let token = sign(&claims(&body));
    let signed = verifier()
        .preauthenticate(
            original.bridge.clone(),
            &keys(),
            CallbackHeaders {
                callback_token: &token,
                user_token: None,
                message_id: "message-1",
                platform: "telegram",
                timestamp: &timestamp(),
            },
            &body,
        )
        .unwrap();
    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(matches!(
        signed.bind(binding(false)),
        Err(CallbackError::Authentication)
    ));
}

#[test]
fn signature_precheck_cannot_be_bound_to_a_different_bridge_snapshot() {
    let original = binding(false);
    let body = serde_json::to_vec(&payload()).unwrap();
    let token = sign(&claims(&body));
    let signed = verifier()
        .preauthenticate(
            original.bridge.clone(),
            &keys(),
            CallbackHeaders {
                callback_token: &token,
                user_token: None,
                message_id: "message-1",
                platform: "telegram",
                timestamp: &timestamp(),
            },
            &body,
        )
        .unwrap();
    let mut another = binding(false);
    another.bridge.id = BridgeId("brg_other".into());
    assert!(matches!(
        signed.bind(another),
        Err(CallbackError::BindingUnavailable)
    ));
}

#[test]
fn dedup_uses_business_message_and_body_hash_while_rotation_requires_known_keys() {
    let write = accept(payload()).unwrap();
    let mut stored = write.record().clone();
    stored.attestation.token_id = "previous-signature".into();
    assert_eq!(write.match_replay(&stored).unwrap(), stored.receipt());
    stored.attestation.body_sha256 = "0".repeat(64);
    assert_eq!(
        write.match_replay(&stored),
        Err(CallbackError::ReplayConflict)
    );
    stored.expires_at = now() - 1;
    assert_eq!(write.match_replay(&stored), Err(CallbackError::Unavailable));
    let mut jwks: Value = serde_json::from_slice(include_bytes!("fixtures/jwks.json")).unwrap();
    let rotation: Value =
        serde_json::from_slice(include_bytes!("fixtures/rotation-jwks.json")).unwrap();
    let rotated = rotation["keys"][0].clone();
    jwks["keys"].as_array_mut().unwrap().push(rotated);
    let rotated = CallbackKeySet::from_pinned_jwks(
        policy().issuer,
        &serde_json::to_vec(&jwks).unwrap(),
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let signed = sign_header(
        json!({"alg":"RS256","typ":"JWT","kid":"rotated-key"}),
        &json!({"fixture":true}),
    );
    let result: Result<(Value, String), _> = rotated.verify(&signed, &policy().issuer);
    assert!(result.is_ok());
    let result: Result<(Value, String), _> = keys().verify(&signed, &policy().issuer);
    assert!(result.is_err());
}
