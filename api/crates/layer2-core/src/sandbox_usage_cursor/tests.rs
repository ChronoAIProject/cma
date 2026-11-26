use super::*;
use crate::{Authorization, Capability};
use std::sync::atomic::{AtomicU64, Ordering};

fn codec(policy: UsageCursorPolicy, time: &Arc<AtomicU64>) -> UsageCursors {
    let time = time.clone();
    UsageCursors::new(&[7; 32], policy).with_clock(Arc::new(move || time.load(Ordering::SeqCst)))
}

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn scope(caller: &Caller) -> Scope<'_> {
    Scope {
        caller,
        admin: false,
        owner: Some(&caller.subject),
        sandbox: None,
        filter_owner: Some(&caller.subject),
    }
}

fn issue(codec: &UsageCursors, scope: &Scope<'_>, last: &str) -> String {
    let chain = codec.begin(scope, &PageRequest::default()).unwrap();
    codec
        .finish(scope, chain, Some(last.into()))
        .unwrap()
        .unwrap()
}

fn page(token: &str) -> PageRequest {
    PageRequest::new(1, Some(token.into())).unwrap()
}

fn rejected(codec: &UsageCursors, scope: &Scope<'_>, token: &str) {
    assert!(matches!(
        codec.begin(scope, &page(token)),
        Err(CursorError::Invalid | CursorError::Store(SandboxStoreError::InvalidPage))
    ));
}

fn resign(codec: &UsageCursors, value: serde_json::Value) -> String {
    let body = match serde_json::from_value::<Payload>(value.clone()) {
        Ok(payload) => serde_json::to_vec(&payload).unwrap(),
        Err(_) => serde_json::to_vec(&value).unwrap(),
    };
    let mut authenticated = b"u1.".to_vec();
    authenticated.extend_from_slice(&body);
    format!(
        "u1.{}.{}",
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(hmac::sign(&codec.key, &authenticated).as_ref())
    )
}

#[test]
fn strict_refuses_raw_and_reader_only_verifies_signed_without_renewing_the_chain() {
    let time = Arc::new(AtomicU64::new(100));
    let issuer = codec(UsageCursorPolicy::Strict, &time);
    let reader = codec(UsageCursorPolicy::ReaderOnly, &time);
    let actor = caller("owner");
    let scope = scope(&actor);
    for raw in ["invalid", "zzz", "sbx_a"] {
        rejected(&issuer, &scope, raw);
        assert_eq!(
            reader
                .begin(&scope, &page(raw))
                .unwrap()
                .page
                .cursor
                .as_deref(),
            Some(raw)
        );
    }
    let token = issue(&issuer, &scope, "sbx_a");
    time.store(900, Ordering::SeqCst);
    let chain = reader.begin(&scope, &page(&token)).unwrap();
    assert_eq!(chain.page.cursor.as_deref(), Some("sbx_a"));
    let next = reader
        .finish(&scope, chain, Some("sbx_b".into()))
        .unwrap()
        .unwrap();
    time.store(999, Ordering::SeqCst);
    assert!(issuer.begin(&scope, &page(&next)).is_ok());
    time.store(1000, Ordering::SeqCst);
    rejected(&reader, &scope, &token);
    rejected(&issuer, &scope, &next);
}

#[test]
fn issue_signed_upgrades_raw_chains_but_reader_only_does_not_start_them() {
    let time = Arc::new(AtomicU64::new(100));
    let actor = caller("owner");
    for policy in [
        UsageCursorPolicy::ReaderOnly,
        UsageCursorPolicy::IssueSigned,
    ] {
        let codec = codec(policy, &time);
        for input in [PageRequest::default(), page("sbx_a")] {
            let chain = codec.begin(&scope(&actor), &input).unwrap();
            let next = codec
                .finish(&scope(&actor), chain, Some("sbx_b".into()))
                .unwrap()
                .unwrap();
            assert_eq!(
                next.starts_with("u1."),
                policy == UsageCursorPolicy::IssueSigned
            );
        }
    }
}

#[test]
fn tokens_bind_principal_mode_effective_owner_endpoint_sandbox_filter_and_order() {
    let time = Arc::new(AtomicU64::new(100));
    let codec = codec(UsageCursorPolicy::Strict, &time);
    let actor = caller("owner");
    let other = caller("other");
    let sandbox = SandboxId("sbx_a".into());
    let other_sandbox = SandboxId("sbx_b".into());
    let token = issue(&codec, &scope(&actor), "sbx_a");
    rejected(
        &codec,
        &Scope {
            caller: &other,
            ..scope(&actor)
        },
        &token,
    );
    rejected(
        &codec,
        &Scope {
            admin: true,
            ..scope(&actor)
        },
        &token,
    );
    rejected(
        &codec,
        &Scope {
            owner: Some(&other.subject),
            ..scope(&actor)
        },
        &token,
    );
    rejected(
        &codec,
        &Scope {
            owner: None,
            ..scope(&actor)
        },
        &token,
    );
    rejected(
        &codec,
        &Scope {
            filter_owner: None,
            ..scope(&actor)
        },
        &token,
    );
    let events = Scope {
        sandbox: Some(&sandbox),
        filter_owner: None,
        ..scope(&actor)
    };
    rejected(&codec, &events, &token);
    let event_token = issue(&codec, &events, "2");
    rejected(&codec, &scope(&actor), &event_token);
    rejected(
        &codec,
        &Scope {
            sandbox: Some(&other_sandbox),
            ..events
        },
        &event_token,
    );
    let decoded = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1).unwrap())
        .unwrap();
    assert!(!String::from_utf8(decoded).unwrap().contains("owner"));
    let replica =
        UsageCursors::new(&[7; 32], UsageCursorPolicy::Strict).with_clock(Arc::new(|| 100));
    assert!(replica.begin(&scope(&actor), &page(&token)).is_ok());
    let other_key =
        UsageCursors::new(&[8; 32], UsageCursorPolicy::Strict).with_clock(Arc::new(|| 100));
    rejected(&other_key, &scope(&actor), &token);
}

#[test]
fn malformed_tampered_unknown_and_oversized_tokens_never_fall_back_to_raw() {
    let time = Arc::new(AtomicU64::new(100));
    let actor = caller("owner");
    let scope = scope(&actor);
    let issuer = codec(UsageCursorPolicy::Strict, &time);
    let token = issue(&issuer, &scope, "sbx_a");
    let mut parts: Vec<_> = token.split('.').map(str::to_string).collect();
    parts[1].replace_range(0..1, "A");
    let invalid = [
        "".into(),
        "u1".into(),
        "u1broken".into(),
        "u2".into(),
        "u2.e30.tag".into(),
        "u1.%%%.tag".into(),
        "u1.a.b.c".into(),
        "u1.你好.tag".into(),
        "u1.e30.tag".into(),
        "x".repeat(4097),
        "x".repeat(257),
        parts.join("."),
        format!("{token}="),
        token.replace("u1.", "u9."),
    ];
    for policy in [
        UsageCursorPolicy::ReaderOnly,
        UsageCursorPolicy::IssueSigned,
        UsageCursorPolicy::Strict,
    ] {
        let codec = codec(policy, &time);
        for bad in &invalid {
            rejected(&codec, &scope, bad);
        }
    }
}

#[test]
fn authenticated_payloads_still_require_version_canonical_shape_time_and_internal_bounds() {
    let time = Arc::new(AtomicU64::new(100));
    let actor = caller("owner");
    let codec = codec(UsageCursorPolicy::Strict, &time);
    let token = issue(&codec, &scope(&actor), "sbx_a");
    let body = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1).unwrap())
        .unwrap();
    let original: serde_json::Value = serde_json::from_slice(&body).unwrap();
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("issued_at", serde_json::json!(101)),
        ("expires_at", serde_json::json!(1001)),
        ("issued_at", serde_json::json!(0)),
        ("last", serde_json::json!("")),
        ("last", serde_json::json!("x".repeat(257))),
        ("extra", serde_json::json!(true)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        rejected(&codec, &scope(&actor), &resign(&codec, changed));
    }
    time.store(99, Ordering::SeqCst);
    rejected(&codec, &scope(&actor), &token);
    time.store(100, Ordering::SeqCst);
    let chain = codec.begin(&scope(&actor), &page(&token)).unwrap();
    time.store(1000, Ordering::SeqCst);
    assert!(codec
        .finish(&scope(&actor), chain, Some("sbx_b".into()))
        .is_err());
}

#[test]
fn limits_can_change_but_revisions_and_internal_keys_remain_bounded() {
    let time = Arc::new(AtomicU64::new(100));
    let actor = caller("owner");
    let codec = codec(UsageCursorPolicy::Strict, &time);
    let token = issue(&codec, &scope(&actor), "sbx_a");
    for limit in [1, 200] {
        assert!(codec
            .begin(
                &scope(&actor),
                &PageRequest {
                    limit,
                    cursor: Some(token.clone())
                }
            )
            .is_ok());
    }
    for limit in [0, 201] {
        assert!(codec
            .begin(
                &scope(&actor),
                &PageRequest {
                    limit,
                    cursor: Some(token.clone())
                }
            )
            .is_err());
    }
    let sandbox = SandboxId("sbx_a".into());
    let events = Scope {
        sandbox: Some(&sandbox),
        ..scope(&actor)
    };
    for key in ["0", "-1", "9223372036854775808", "not-a-revision"] {
        let chain = codec.begin(&events, &PageRequest::default()).unwrap();
        assert!(matches!(
            codec.finish(&events, chain, Some(key.into())),
            Err(CursorError::Store(SandboxStoreError::Corrupt(_)))
        ));
    }
}

#[test]
fn snapshots_require_confirmed_identity_and_continue_on_reader_only_until_original_expiry() {
    let time = Arc::new(AtomicU64::new(100));
    let actor = caller("owner");
    let scope = scope(&actor);
    let issuer = codec(UsageCursorPolicy::Strict, &time).with_snapshot_issuance(true);
    let reader = codec(UsageCursorPolicy::ReaderOnly, &time);
    let legacy = codec(UsageCursorPolicy::IssueSigned, &time);
    let snapshot = UsageSnapshot {
        time: 80,
        increment: 17,
    };
    let mut first = issuer.begin(&scope, &PageRequest::default()).unwrap();
    assert!(matches!(first.view, PageView::Snapshot(None)));
    assert_eq!(first.as_of(), Some(100));
    first.confirm_snapshot(snapshot).unwrap();
    let token = issuer
        .finish(&scope, first, Some("sbx_a".into()))
        .unwrap()
        .unwrap();
    assert!(token.starts_with("u2."));
    let wrong_version = token.replacen("u2.", "u1.", 1);
    rejected(&reader, &scope, &wrong_version);
    let mut parts: Vec<_> = token.split('.').map(str::to_string).collect();
    let body = URL_SAFE_NO_PAD.decode(&parts[1]).unwrap();
    let mut payload: Payload = serde_json::from_slice(&body).unwrap();
    payload.snapshot.as_mut().unwrap().increment += 1;
    parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    rejected(&reader, &scope, &parts.join("."));
    time.store(900, Ordering::SeqCst);
    let mut second = reader.begin(&scope, &page(&token)).unwrap();
    assert!(matches!(second.view, PageView::Snapshot(Some(value)) if value == snapshot));
    assert_eq!(second.as_of(), Some(100));
    assert!(second
        .confirm_snapshot(UsageSnapshot {
            increment: 18,
            ..snapshot
        })
        .is_err());
    second.confirm_snapshot(snapshot).unwrap();
    let next = reader
        .finish(&scope, second, Some("sbx_b".into()))
        .unwrap()
        .unwrap();
    time.store(999, Ordering::SeqCst);
    let mut last = issuer.begin(&scope, &page(&next)).unwrap();
    last.confirm_snapshot(snapshot).unwrap();
    time.store(1000, Ordering::SeqCst);
    assert!(issuer.finish(&scope, last, None).is_err());
    rejected(&reader, &scope, &next);
    let legacy_token = issue(&legacy, &scope, "sbx_a");
    let chain = issuer.begin(&scope, &page(&legacy_token)).unwrap();
    assert!(matches!(chain.view, PageView::Live));
    assert!(issuer
        .finish(&scope, chain, Some("sbx_b".into()))
        .unwrap()
        .unwrap()
        .starts_with("u1."));
}
