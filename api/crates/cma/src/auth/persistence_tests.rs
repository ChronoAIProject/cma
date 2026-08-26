use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::*;

fn subject_state(revision: u64) -> SubjectAuthState {
    SubjectAuthState {
        subject: SubjectId("subject-a".into()),
        incarnation: "subject-incarnation-a".into(),
        revision,
        token_generation: 0,
        tokens: None,
        refresh_lease: None,
        authorization: Authorization::none(),
        authorization_check_after: SystemTime::UNIX_EPOCH,
        profile: Profile::default(),
    }
}

fn credential_state(generation: u64) -> SessionCredentialState {
    SessionCredentialState {
        session: SessionId("sesn_a".into()),
        generation,
        request_bound_credential: None,
        current: None,
        pending: None,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
    }
}

fn subject_tokens(expires_at: SystemTime, refresh: Option<&str>) -> DurableSubjectTokens {
    DurableSubjectTokens {
        access_token: SecretValue::new("access".into()),
        refresh_token: refresh.map(|value| SecretValue::new(value.into())),
        expires_at,
    }
}

fn credential(id: &str, issued_at: SystemTime) -> DurableCredential {
    DurableCredential {
        id: id.into(),
        value: SecretValue::new(format!("value-{id}")),
        owner: SubjectId("owner".into()),
        issued_at,
        expires_at: None,
        scope_fingerprint: "scope".into(),
        revocation_credential: None,
    }
}

#[test]
fn secret_diagnostics_are_redacted() {
    let secret = SecretValue::new("plaintext-under-test".into());
    let rendered = format!("{secret:?}");
    assert!(rendered.contains("redacted"));
    assert!(!rendered.contains(secret.expose()));
}

#[test]
fn record_domains_produce_distinct_identifiers() {
    let value = "opaque-random-value";
    assert_ne!(
        SecretDigest::identify("browser-session", value),
        SecretDigest::identify("login-attempt", value)
    );
}

#[tokio::test]
async fn two_instances_share_browser_sessions_and_consume_login_once() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = store.clone() as Arc<dyn AuthStore>;
    let second = store as Arc<dyn AuthStore>;
    let browser_id = SecretDigest::identify("browser-session", "cookie");
    first
        .put_browser_session(BrowserSession {
            id: browser_id.clone(),
            subject: SubjectId("subject-a".into()),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        })
        .await
        .unwrap();
    assert!(second.browser_session(&browser_id).await.unwrap().is_some());

    let login_id = SecretDigest::identify("login-attempt", "state");
    first
        .put_login_attempt(LoginAttempt {
            id: login_id.clone(),
            verifier: SecretValue::new("verifier".into()),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        })
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        first.take_login_attempt(&login_id),
        second.take_login_attempt(&login_id)
    );
    let winners = [left.unwrap(), right.unwrap()]
        .into_iter()
        .filter(Option::is_some)
        .count();
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn subject_compare_and_swap_has_one_winner() {
    let store = Arc::new(MemoryAuthStore::default());
    assert!(store.insert_subject(subject_state(0)).await.unwrap());
    let (left, right) = tokio::join!(
        store.replace_subject("subject-incarnation-a", 0, subject_state(1)),
        store.replace_subject("subject-incarnation-a", 0, subject_state(1))
    );
    assert_eq!(
        [left.unwrap(), right.unwrap()]
            .into_iter()
            .filter(|won| *won)
            .count(),
        1
    );
}

#[tokio::test]
async fn subject_compare_and_swap_upgrades_an_empty_legacy_incarnation() {
    let store = MemoryAuthStore::default();
    let mut legacy = subject_state(0);
    legacy.incarnation.clear();
    let subject = legacy.subject.clone();
    assert!(store.insert_subject(legacy.clone()).await.unwrap());

    legacy.revision = 1;
    assert!(store.replace_subject("", 0, legacy).await.unwrap());
    assert!(!store
        .subject(&subject)
        .await
        .unwrap()
        .unwrap()
        .incarnation
        .is_empty());
}

#[tokio::test]
async fn subject_compare_and_swap_rejects_ttl_recreation_revision_aba() {
    let store = MemoryAuthStore::default();
    let original = subject_state(0);
    let subject = original.subject.clone();
    assert!(store.insert_subject(original.clone()).await.unwrap());

    store.subjects.lock().await.remove(&subject);
    store.subject_sweep_sequences.lock().await.remove(&subject);
    let mut replacement = subject_state(0);
    replacement.incarnation = "replacement-incarnation".into();
    replacement.profile.display_name = Some("replacement".into());
    assert!(store.insert_subject(replacement).await.unwrap());

    let mut stale_write = original;
    stale_write.revision = 1;
    stale_write.profile.display_name = Some("stale".into());
    assert!(!store
        .replace_subject("subject-incarnation-a", 0, stale_write)
        .await
        .unwrap());
    let retained = store.subject(&subject).await.unwrap().unwrap();
    assert_eq!(retained.incarnation, "replacement-incarnation");
    assert_eq!(
        retained.profile.display_name.as_deref(),
        Some("replacement")
    );
}

#[tokio::test]
async fn credential_compare_and_swap_rejects_a_stale_worker() {
    let store = MemoryAuthStore::default();
    assert!(store.insert_credential(credential_state(0)).await.unwrap());
    assert!(store
        .replace_credential(0, credential_state(1))
        .await
        .unwrap());
    assert!(!store
        .replace_credential(0, credential_state(1))
        .await
        .unwrap());
    assert_eq!(
        store
            .credential(&SessionId("sesn_a".into()))
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
}

#[tokio::test]
async fn due_subject_pages_follow_each_index_order_and_keep_lease_lanes_disjoint() {
    let store = MemoryAuthStore::default();
    let now = SystemTime::now();
    for id in ["subject-a", "subject-b", "subject-c"] {
        let mut state = subject_state(0);
        state.subject = SubjectId(id.into());
        state.tokens = Some(subject_tokens(
            now - Duration::from_secs(1),
            Some("refresh"),
        ));
        assert!(store.insert_subject(state).await.unwrap());
    }

    let mut future = subject_state(0);
    future.subject = SubjectId("subject-future".into());
    future.tokens = Some(subject_tokens(
        now + Duration::from_secs(600),
        Some("refresh"),
    ));
    assert!(store.insert_subject(future).await.unwrap());

    let mut stranded = subject_state(0);
    stranded.subject = SubjectId("subject-stranded".into());
    stranded.tokens = Some(subject_tokens(now - Duration::from_secs(1), None));
    assert!(store.insert_subject(stranded).await.unwrap());

    let mut leased = subject_state(0);
    leased.subject = SubjectId("subject-leased".into());
    leased.tokens = Some(subject_tokens(
        now - Duration::from_secs(1),
        Some("refresh"),
    ));
    leased.refresh_lease = Some(SubjectRefreshLease {
        holder: "worker".into(),
        expires_at: now + Duration::from_secs(45),
    });
    assert!(store.insert_subject(leased).await.unwrap());

    let mut expired_lease = subject_state(0);
    expired_lease.subject = SubjectId("subject-expired-lease".into());
    expired_lease.tokens = Some(subject_tokens(
        now - Duration::from_secs(1),
        Some("refresh"),
    ));
    expired_lease.refresh_lease = Some(SubjectRefreshLease {
        holder: "expired-worker".into(),
        expires_at: now - Duration::from_secs(2),
    });
    assert!(store.insert_subject(expired_lease).await.unwrap());

    let mut tokenless = subject_state(0);
    tokenless.subject = SubjectId("subject-tokenless".into());
    assert!(store.insert_subject(tokenless).await.unwrap());
    let through_sequence = store.auth_sweep_high_water().await.unwrap();

    let first = store
        .unleased_subjects_due_for_refresh(now, None, through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|item| item.state.as_ref().unwrap().subject.as_str())
            .collect::<Vec<_>>(),
        ["subject-a", "subject-b"]
    );
    let cursor = first[1].cursor.clone();
    let second = store
        .unleased_subjects_due_for_refresh(now, Some(&cursor), through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].state.as_ref().unwrap().subject.as_str(),
        "subject-c"
    );

    let expired = store
        .subjects_with_expired_refresh_leases(now, None, through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(
        expired[0].state.as_ref().unwrap().subject.as_str(),
        "subject-expired-lease"
    );

    let token_subjects = store
        .subjects_with_tokens(None, through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(
        token_subjects
            .iter()
            .map(|state| state.subject.as_str())
            .collect::<Vec<_>>(),
        ["subject-a", "subject-b"]
    );
    let next_token_subjects = store
        .subjects_with_tokens(Some(&token_subjects[1]), through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(
        next_token_subjects
            .iter()
            .map(|state| state.subject.as_str())
            .collect::<Vec<_>>(),
        ["subject-c", "subject-future"]
    );
    let all_token_subjects = store
        .subjects_with_tokens(None, through_sequence, 10)
        .await
        .unwrap();
    assert!(all_token_subjects
        .iter()
        .any(|cursor| cursor.subject.as_str() == "subject-stranded"));
    assert!(store
        .subjects_with_tokens(None, through_sequence, 0)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn authorization_due_pages_order_by_deadline_then_subject() {
    let store = MemoryAuthStore::default();
    let now = SystemTime::now();
    let first_deadline = now - Duration::from_secs(2);
    for (id, deadline) in [
        ("subject-b", first_deadline),
        ("subject-a", first_deadline),
        ("subject-c", now - Duration::from_secs(1)),
        ("subject-future", now + Duration::from_secs(1)),
    ] {
        let mut state = subject_state(0);
        state.subject = SubjectId(id.into());
        state.tokens = Some(subject_tokens(now + Duration::from_secs(600), None));
        state.authorization_check_after = deadline;
        assert!(store.insert_subject(state).await.unwrap());
    }
    let mut tokenless = subject_state(0);
    tokenless.subject = SubjectId("subject-tokenless".into());
    tokenless.authorization_check_after = first_deadline;
    assert!(store.insert_subject(tokenless).await.unwrap());

    let first = store
        .subjects_due_for_authorization_check(now, None, 2)
        .await
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|item| item.cursor.subject.as_str())
            .collect::<Vec<_>>(),
        ["subject-a", "subject-b"]
    );
    let second = store
        .subjects_due_for_authorization_check(now, Some(&first[1].cursor), 2)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].cursor.subject.as_str(), "subject-c");
    assert_eq!(second[0].cursor.ready_at, Some(now - Duration::from_secs(1)));
    assert!(store
        .subjects_due_for_authorization_check(now, None, 0)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn credential_work_pages_are_bounded_and_skip_only_settled_history() {
    let store = MemoryAuthStore::default();
    let now = SystemTime::now();
    let old = now - Duration::from_secs(120);

    let mut first = credential_state(0);
    first.session = SessionId("sesn_a".into());
    first.current = Some(credential("current-a", old));
    first.revoke_debt = vec![
        RevokeDebt {
            id: "debt-a".into(),
            owner: SubjectId("owner".into()),
            queued_at: old,
            revocation_credential: None,
        },
        RevokeDebt {
            id: "debt-b".into(),
            owner: SubjectId("owner".into()),
            queued_at: old,
            revocation_credential: None,
        },
    ];
    assert!(store.insert_credential(first).await.unwrap());

    let mut pending = credential_state(0);
    pending.session = SessionId("sesn_b".into());
    pending.pending = Some(credential("pending-b", now));
    assert!(store.insert_credential(pending).await.unwrap());

    let mut second_old = credential_state(0);
    second_old.session = SessionId("sesn_aa".into());
    second_old.current = Some(credential("current-aa", old));
    assert!(store.insert_credential(second_old).await.unwrap());

    let mut fresh = credential_state(0);
    fresh.session = SessionId("sesn_c".into());
    fresh.current = Some(credential("current-c", now));
    assert!(store.insert_credential(fresh).await.unwrap());

    let mut settled = credential_state(0);
    settled.session = SessionId("sesn_d".into());
    settled.retired = true;
    assert!(store.insert_credential(settled).await.unwrap());

    let mut retired_debt = credential_state(0);
    retired_debt.session = SessionId("sesn_e".into());
    retired_debt.retired = true;
    retired_debt.revoke_debt.push(RevokeDebt {
        id: "debt-e".into(),
        owner: SubjectId("owner".into()),
        queued_at: old,
        revocation_credential: None,
    });
    assert!(store.insert_credential(retired_debt).await.unwrap());
    let through_sequence = store.auth_sweep_high_water().await.unwrap();

    let first_due = store
        .current_credentials_due_for_reissue(
            now - Duration::from_secs(60),
            None,
            through_sequence,
            1,
        )
        .await
        .unwrap();
    assert_eq!(first_due.len(), 1);
    assert_eq!(
        first_due[0].state.as_ref().unwrap().session.as_str(),
        "sesn_a"
    );
    let cursor = first_due[0].cursor.clone();
    let next_due = store
        .current_credentials_due_for_reissue(
            now - Duration::from_secs(60),
            Some(&cursor),
            through_sequence,
            10,
        )
        .await
        .unwrap();
    assert_eq!(next_due.len(), 1);
    assert_eq!(
        next_due[0].state.as_ref().unwrap().session.as_str(),
        "sesn_aa"
    );
    let pending_due = store
        .pending_credentials_due_for_reissue(None, through_sequence, 10)
        .await
        .unwrap();
    assert_eq!(pending_due.len(), 1);
    assert_eq!(
        pending_due[0].state.as_ref().unwrap().session.as_str(),
        "sesn_b"
    );

    let first_debts = store.revoke_debts(None, through_sequence, 2).await.unwrap();
    assert_eq!(first_debts.len(), 2);
    assert_eq!(first_debts[0].state.as_ref().unwrap().debt.id, "debt-a");
    assert_eq!(first_debts[1].state.as_ref().unwrap().debt.id, "debt-b");
    let cursor = first_debts[1].cursor.clone();
    let next_debts = store
        .revoke_debts(Some(&cursor), through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(next_debts.len(), 1);
    assert_eq!(next_debts[0].state.as_ref().unwrap().debt.id, "debt-e");
    let reconciliation = store
        .active_credentials_for_reconciliation(None, through_sequence, 10)
        .await
        .unwrap();
    assert_eq!(
        reconciliation
            .iter()
            .map(|state| state.session.as_str())
            .collect::<Vec<_>>(),
        ["sesn_a", "sesn_b", "sesn_aa", "sesn_c"]
    );
}

#[tokio::test]
async fn expired_mint_lease_pages_follow_deadline_and_session_order() {
    let store = MemoryAuthStore::default();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let first_deadline = now - Duration::from_secs(120);
    let second_deadline = now - Duration::from_secs(60);

    for (session, deadline) in [
        ("sesn_a", first_deadline),
        ("sesn_b", first_deadline),
        ("sesn_c", second_deadline),
    ] {
        let mut leased = credential_state(0);
        leased.session = SessionId(session.into());
        leased.current = Some(credential(&format!("current-{session}"), first_deadline));
        leased.lease = Some(CredentialLease {
            holder: "abandoned-worker".into(),
            fence: 1,
            acquired_at: first_deadline,
            expires_at: deadline,
            scope_fingerprint: "scope".into(),
        });
        assert!(store.insert_credential(leased).await.unwrap());
    }

    let mut active = credential_state(0);
    active.session = SessionId("sesn_active".into());
    active.current = Some(credential("current-active", first_deadline));
    active.lease = Some(CredentialLease {
        holder: "active-worker".into(),
        fence: 1,
        acquired_at: now,
        expires_at: now + Duration::from_secs(60),
        scope_fingerprint: "scope".into(),
    });
    assert!(store.insert_credential(active).await.unwrap());
    let through_sequence = store.auth_sweep_high_water().await.unwrap();

    let first = store
        .credentials_with_expired_mint_leases(now, None, through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|item| item.state.as_ref().unwrap().session.as_str())
            .collect::<Vec<_>>(),
        ["sesn_a", "sesn_b"]
    );
    let cursor = first[1].cursor.clone();
    let next = store
        .credentials_with_expired_mint_leases(now, Some(&cursor), through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].state.as_ref().unwrap().session.as_str(), "sesn_c");
}

#[tokio::test]
async fn subject_revision_overflow_is_rejected_instead_of_saturating() {
    let store = MemoryAuthStore::default();
    store
        .subjects
        .lock()
        .await
        .insert(SubjectId("subject-a".into()), subject_state(u64::MAX));

    assert!(matches!(
        store
            .replace_subject("subject-incarnation-a", u64::MAX, subject_state(u64::MAX),)
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn credential_generation_overflow_is_rejected_instead_of_saturating() {
    let store = MemoryAuthStore::default();
    store
        .credentials
        .lock()
        .await
        .insert(SessionId("sesn_a".into()), credential_state(u64::MAX));

    assert!(matches!(
        store
            .replace_credential(u64::MAX, credential_state(u64::MAX))
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn store_failure_is_distinct_from_absent_state() {
    let store = MemoryAuthStore::default();
    store.set_unavailable(true);
    assert!(matches!(
        store
            .browser_session(&SecretDigest::identify("browser-session", "missing"))
            .await,
        Err(StoreError::Unavailable(_))
    ));
}
