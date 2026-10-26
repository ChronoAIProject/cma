use super::{
    test_server::{Row, Server},
    tests::fixture,
    types::*,
    *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::{seal_consumer_interaction, MasterKeyRing};
use std::sync::{atomic::Ordering, Arc};

#[tokio::test]
async fn replay_restores_admission_before_current_routing_or_credentials_are_considered() {
    let server = Server::start().await;
    let mut request = fixture::request(fixture::targets, fixture::message("hello"));
    let upload = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"source",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let original = server.store().admit(&upload).await.unwrap();
    let definition = &mut Arc::get_mut(&mut request.channel).unwrap().definition;
    definition.routing = None;
    definition.secrets.consumer_keys.clear();
    assert_eq!(
        server.store().replay(&request, b"source").await.unwrap(),
        Some(original)
    );
    assert_eq!(
        server
            .store()
            .replay(&request, b"changed")
            .await
            .unwrap_err(),
        AdmissionError::Conflict
    );
}

#[tokio::test]
async fn relay_reply_authority_is_retained_only_inside_encrypted_payload() {
    let server = Server::start().await;
    let mut activity = fixture::message("hello");
    activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
        platform: cmaeg_core::channel::Platform::Lark,
        route_id: fixture::ROUTE.into(),
    };
    let request = fixture::request(fixture::targets, activity);
    let planned = fixture::routed(&request);
    let upload = Candidate::prepare(
        &request,
        &planned,
        b"relay body",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let restored = server.store().admit(&upload).await.unwrap();
    for (target, original) in restored.targets.iter().zip(&planned.targets) {
        assert_eq!(target.launch.event, original.event);
    }
    assert!(!format!("{restored:?}").contains("reply-fixture"));
    assert!(server
        .memory
        .rows
        .lock()
        .unwrap()
        .values()
        .all(|row| !String::from_utf8_lossy(&row.blob).contains("reply-fixture")));
}

#[test]
fn candidates_cannot_extend_event_authority_or_replace_source_or_provider() {
    let mut request = fixture::request(fixture::targets, fixture::message("hello"));
    fixture::rewrite_ref(&mut request, |event| event.expires_at = fixture::NOW + 60);
    let original = fixture::routed(&request);
    for change in ["expiry", "source", "provider", "record"] {
        let mut plan = original.clone();
        match change {
            "expiry" => plan.targets[0].event.expires_at += 1,
            "source" => {
                plan.source = cmaeg_core::channel::Source::Custom {
                    auth: cmaeg_core::channel::CustomAuth::Hmac {
                        key_id: "changed".into(),
                    },
                    prompt_template: None,
                    schema: None,
                    attachments: None,
                }
            }
            "provider" => {
                plan.targets[0].provider.binding_id =
                    cmaeg_core::ids::BindingId::new("other-binding").unwrap()
            }
            _ => {
                plan.targets[0].event.record_id =
                    cmaeg_core::ids::RecordId::new("other-record").unwrap()
            }
        }
        assert!(
            Candidate::prepare(&request, &plan, b"source", &fixture::keys(), fixture::NOW).is_err(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn record_key_rotation_preserves_snapshot_with_retained_decryption_key() {
    let server = Server::start().await;
    let request = fixture::request(
        fixture::targets,
        fixture::message("private retained source"),
    );
    let upload = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"private retained source",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let expected = server.store().admit(&upload).await.unwrap();
    assert!(server
        .memory
        .rows
        .lock()
        .unwrap()
        .values()
        .all(|row| !String::from_utf8_lossy(&row.blob).contains("private retained source")));
    let mut rotated = server.store();
    rotated.keys = Arc::new(
        MasterKeyRing::parse(&format!(
            "2:{},1:{}",
            STANDARD.encode([8; 32]),
            STANDARD.encode([7; 32])
        ))
        .unwrap(),
    );
    assert_eq!(
        rotated.load(upload.identity()).await.unwrap().unwrap(),
        expected
    );
    rotated.keys =
        Arc::new(MasterKeyRing::parse(&format!("2:{}", STANDARD.encode([8; 32]))).unwrap());
    assert_eq!(
        rotated.load(upload.identity()).await.unwrap_err(),
        AdmissionError::Corrupt
    );
}

#[tokio::test]
async fn recovery_skips_named_cards_and_reports_corrupt_committed_payload() {
    let server = Server::start().await;
    let request = fixture::request(fixture::targets, fixture::message("hello"));
    let upload = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"source",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    server.store().admit(&upload).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let blob = seal_consumer_interaction(&fixture::keys(), &id, b"{}").unwrap();
    server.memory.rows.lock().unwrap().insert(
        id,
        Row {
            owner: upload.identity().owner.clone(),
            blob,
            expiry: upload.manifest.retained_until,
            version: 9,
        },
    );
    let page = server
        .store()
        .recover_page(&upload.identity().owner, None, 32)
        .await
        .unwrap();
    assert_eq!(page.plans.len(), 1);
    assert!(page.issues.is_empty());
    server
        .memory
        .rows
        .lock()
        .unwrap()
        .remove(&upload.manifest.chunk_id(0));
    let page = server
        .store()
        .recover_page(&upload.identity().owner, None, 32)
        .await
        .unwrap();
    assert!(page.plans.is_empty());
    assert_eq!(page.issues.len(), 1);
    assert_eq!(page.issues[0].record_id, upload.admission_id());
    assert_eq!(page.issues[0].error, AdmissionError::MissingChunk);
}

#[tokio::test]
async fn expired_hidden_unique_row_is_not_deleted_or_recreated() {
    let server = Server::start().await;
    let request = fixture::request(fixture::targets, fixture::message("hello"));
    let upload = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"source",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    server.store().admit(&upload).await.unwrap();
    server
        .memory
        .rows
        .lock()
        .unwrap()
        .get_mut(upload.admission_id())
        .unwrap()
        .expiry = fixture::NOW;
    let writes = server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        server.store().admit(&upload).await.unwrap_err(),
        AdmissionError::ExpiredOrUnavailable
    );
    assert_eq!(server.memory.writes.load(Ordering::SeqCst), writes);
    assert!(server
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(upload.admission_id()));
}
