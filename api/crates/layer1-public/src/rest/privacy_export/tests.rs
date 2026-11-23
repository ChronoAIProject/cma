use super::*;
use crate::rest::{
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use layer2_core::{
    account_deletion::{AccountDeletionError, AccountStanding, AvailableAccounts},
    resource_access::{ResourceAccessService, ResourceShare, ShareChange, ShareCommand},
    sandbox::{
        store::{AgentLogStore, SandboxStore, SandboxStoreError, WorkspaceStore},
        Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentTranscript, AgentTranscriptMessage,
        AgentTranscriptPage, AgentTranscriptRole, AgentTranscriptTurn, AgentTurnOutcome,
        TypedFailure,
    },
    SubjectId,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;
use tower::ServiceExt;

const INVENTORY: &str = "/api/v1/account/privacy-export";
const TRANSCRIPT: &str = "/api/v1/account/privacy-export/agents/agt_shared_read/transcript";
const USER_TEXT: &str = "Recorded café message";

pub(in crate::rest) async fn contract_scenario(
    name: &str,
) -> Option<crate::rest::public_contract_tests::Scenario> {
    let mode = name.strip_prefix("privacy_export_")?;
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let source = Arc::new(Source {
        absent: mode == "legacy",
        large: mode == "large",
        gap: mode == "gap",
        ..Default::default()
    });
    let router = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source,
    )));
    let router = if mode == "unverified" || mode == "anonymous" {
        router
    } else {
        router.layer(Extension(OwnerSessionAuthority::verified_personal(
            SubjectId::new("owner-a"),
        )))
    };
    Some(crate::rest::public_contract_tests::Scenario {
        router,
        subject: (mode != "anonymous").then_some("owner-a"),
        authorization: layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}

#[derive(Default)]
struct Source {
    reads: AtomicUsize,
    legacy_reads: AtomicUsize,
    absent: bool,
    large: bool,
    gap: bool,
    blocked: bool,
    entered: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl AgentLogStore for Source {
    async fn append_agent_log(
        &self,
        _: &AgentId,
        _: String,
        _: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        panic!("export must never write")
    }

    async fn agent_log(
        &self,
        _: &AgentId,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.legacy_reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }

    async fn agent_log_export_page(
        &self,
        agent: &Agent,
        _: &AgentTranscriptPageRequest,
    ) -> Result<Option<AgentTranscriptPage>, SandboxStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.blocked {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        if self.absent {
            return Ok(None);
        }
        let timestamp = "2026-01-02T03:04:05Z";
        let content = if self.large {
            "x".repeat(1024 * 1024)
        } else {
            USER_TEXT.to_owned()
        };
        Ok(Some(AgentTranscriptPage {
            transcript: AgentTranscript {
                agent_id: agent.id.clone(),
                through_seq: agent.conversation.last_seq,
                turns: vec![AgentTranscriptTurn {
                    retained_output: None,
                    turn_sequence: 3,
                    started_at: timestamp.into(),
                    origin_key: Some("private-origin".into()),
                    owner_message: USER_TEXT.into(),
                    agent_messages: vec![],
                    messages: vec![AgentTranscriptMessage {
                        sequence: 5,
                        role: AgentTranscriptRole::Owner,
                        content,
                        appended_at: timestamp.into(),
                    }],
                    metadata: [("private".into(), "private-actor".into())].into(),
                    outcome: AgentTurnOutcome::Failed {
                        at: timestamp.into(),
                        failure: TypedFailure {
                            class: if self.gap {
                                "conversation_output_gap"
                            } else {
                                "runtime_failed"
                            }
                            .into(),
                            detail: "private-diagnostic".into(),
                        },
                    },
                }],
            },
            next: None,
        }))
    }
}

#[derive(Default)]
struct Standing {
    checks: AtomicUsize,
    erasing: AtomicBool,
    entered: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl AccountStanding for Standing {
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        if self.checks.fetch_add(1, Ordering::SeqCst) == 1 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(self.erasing.load(Ordering::SeqCst))
    }
}

fn request(path: &str, subject: &str, owner_session: bool) -> Request<Body> {
    let mut request = agents_tests::request(Method::GET, path, subject, None, None);
    if owner_session {
        request
            .extensions_mut()
            .insert(OwnerSessionAuthority::verified_personal(SubjectId::new(
                subject,
            )));
    }
    request
}

async fn response(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

async fn revoke(access: &ResourceAccessService, share: &ResourceShare) {
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-account-export".into(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn personal_inventory_and_payload_exclude_other_authority_and_private_fields() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    fixture
        .store
        .create_sandbox(agents_tests::sandbox("sbx_other", "recipient"))
        .await
        .unwrap();
    fixture
        .store
        .create_workspace(agents_tests::workspace(
            "wks_other",
            "sbx_other",
            "recipient",
        ))
        .await
        .unwrap();
    let mut foreign = agents_tests::seeded_agent("agt_other", AgentStatus::Ready, 2, None);
    foreign.owner = SubjectId::new("recipient");
    foreign.sandbox_id = layer2_core::SandboxId("sbx_other".into());
    foreign.workspace_id = layer2_core::WorkspaceId("wks_other".into());
    agents_tests::insert_agent(&fixture.store, foreign, vec![]).await;

    let source = Arc::new(Source::default());
    let app = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source.clone(),
    )));
    for (subject, present, absent) in [
        ("owner-a", "agt_shared_read", "agt_other"),
        ("recipient", "agt_other", "agt_shared_read"),
    ] {
        let (status, inventory) = response(app.clone(), request(INVENTORY, subject, true)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(inventory.to_string().contains(present));
        assert!(!inventory.to_string().contains(absent));
    }
    let (status, withheld) = response(app.clone(), request(TRANSCRIPT, "recipient", true)).await;
    assert!(status.is_client_error());
    assert!(!withheld.to_string().contains(USER_TEXT));
    let (status, _) = response(app.clone(), request(TRANSCRIPT, "owner-a", false)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);

    let (status, data) = response(app, request(TRANSCRIPT, "owner-a", true)).await;
    assert_eq!(status, StatusCode::OK);
    let payload = data["payload"].as_str().unwrap();
    assert!(payload.contains(USER_TEXT));
    assert_eq!(data["manifest"]["size_bytes"], payload.len());
    assert_eq!(data["manifest"]["records"], 2);
    assert_eq!(
        data["manifest"]["sha256"],
        format!("{:x}", Sha256::digest(payload.as_bytes()))
    );
    assert_eq!(
        data["manifest"]["coverage"]["completeness"],
        "not_certified"
    );
    assert!(!data.to_string().contains("private-actor"));
    assert!(!data.to_string().contains("private-origin"));
    assert!(!data.to_string().contains("private-diagnostic"));
}

#[tokio::test]
async fn route_refuses_unsupported_legacy_source_without_full_read() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let source = Arc::new(Source {
        absent: true,
        ..Default::default()
    });
    let app = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source.clone(),
    )));
    let (status, body) = response(app, request(TRANSCRIPT, "owner-a", true)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "privacy_export_source_unavailable");
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    assert_eq!(source.legacy_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn transcript_marks_known_output_gaps_and_attests_each_requested_window() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let source = Arc::new(Source {
        gap: true,
        ..Default::default()
    });
    let app = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source,
    )));
    let first = response(app.clone(), request(TRANSCRIPT, "owner-a", true))
        .await
        .1;
    assert_eq!(
        first["manifest"]["window"],
        serde_json::json!({
            "turn": null, "after": null, "through": null, "limit": 32,
        })
    );
    let continued = format!("{TRANSCRIPT}?agent=agt_shared_read&turn=3&after=4&through=9&limit=8");
    let (status, final_page) = response(app, request(&continued, "owner-a", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        final_page["manifest"]["window"],
        serde_json::json!({
            "turn": 3, "after": 4, "through": 9, "limit": 8,
        })
    );
    assert_eq!(final_page["manifest"]["next"], Value::Null);
    let payload = final_page["payload"].as_str().unwrap();
    let turn: Value = serde_json::from_str(payload.lines().next().unwrap()).unwrap();
    assert_eq!(turn["known_output_gap"], true);
    assert_eq!(turn["outcome"], "failed");
    assert!(!payload.contains("private-diagnostic"));
}

#[tokio::test]
async fn transcript_refuses_stale_and_oversized_pages() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let source = Arc::new(Source {
        large: true,
        ..Default::default()
    });
    let app = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source.clone(),
    )));
    let stale = format!("{TRANSCRIPT}?agent=agt_shared_read&turn=3&after=5&through=8&limit=32");
    let (status, _) = response(app.clone(), request(&stale, "owner-a", true)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let (status, body) = response(app, request(TRANSCRIPT, "owner-a", true)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "privacy_export_page_too_large");
}

#[tokio::test]
async fn transcript_withholds_bytes_after_share_revocation_during_source_read() {
    let (fixture, access, _, share) = fixture(Reply::Member).await;
    let source = Arc::new(Source {
        blocked: true,
        ..Default::default()
    });
    let app = router(Arc::new(PrivacyExport::new(
        fixture.reads(),
        Arc::new(AvailableAccounts),
        source.clone(),
    )));
    let pending = tokio::spawn(response(app, request(TRANSCRIPT, "owner-a", true)));
    tokio::time::timeout(Duration::from_secs(3), source.entered.notified())
        .await
        .unwrap();
    revoke(&access, &share).await;
    source.resume.notify_one();
    let (status, body) = pending.await.unwrap();
    assert!(status.is_client_error());
    assert!(!body.to_string().contains(USER_TEXT));
}

#[tokio::test]
async fn inventory_withholds_prepared_rows_on_standing_change_or_share_revocation() {
    for erase in [false, true] {
        let (fixture, access, _, share) = fixture(Reply::Member).await;
        let standing = Arc::new(Standing::default());
        let app = router(Arc::new(PrivacyExport::new(
            fixture.reads(),
            standing.clone(),
            Arc::new(Source::default()),
        )));
        let pending = tokio::spawn(response(app, request(INVENTORY, "owner-a", true)));
        tokio::time::timeout(Duration::from_secs(3), standing.entered.notified())
            .await
            .unwrap();
        if erase {
            standing.erasing.store(true, Ordering::SeqCst);
        } else {
            revoke(&access, &share).await;
        }
        standing.resume.notify_one();
        let (status, body) = pending.await.unwrap();
        assert!(status.is_client_error());
        assert!(!body.to_string().contains("agt_shared_read"));
        assert!(!body.to_string().contains("agt_sibling"));
    }
}
