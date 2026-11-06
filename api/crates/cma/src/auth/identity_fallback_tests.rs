use super::*;
use crate::auth::{permissions::PermissionEvidence, profile::Profile};

fn evidence() -> Arc<VerifiedSubject> {
    Arc::new(VerifiedSubject {
        subject: layer2_core::SubjectId::new("fixture"),
        permissions: PermissionEvidence::Active(vec![]),
        profile: Profile::default(),
        independent_commands: false,
        api_expires_at: None,
        token_expires_at: None,
    })
}

fn seed(store: &MemoryIdentityStore, token: &str) -> TokenHash {
    let key = store.key(CredentialPurpose::BrowserCookie, token);
    store.remember(
        key,
        &store.epoch(),
        evidence(),
        SystemTime::now() + Duration::from_secs(3600),
    );
    key
}

#[tokio::test(start_paused = true)]
async fn fallback_never_renews_the_real_verification_time() {
    let store = MemoryIdentityStore::default();
    let key = seed(&store, "fixture-token");
    for _ in 0..60 {
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(store.get(&key, &store.epoch()).is_some());
    }
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(store.get(&key, &store.epoch()).is_none());
    assert!(store.records.lock().unwrap().entries.is_empty());
    seed(&store, "fixture-token");
    assert!(store.get(&key, &store.epoch()).is_some());
}

#[test]
fn expiry_and_missing_lifetime_cannot_extend_identity() {
    let store = MemoryIdentityStore::default();
    let key = seed(&store, "fixture-token");
    store
        .records
        .lock()
        .unwrap()
        .entries
        .get_mut(&key)
        .unwrap()
        .expires_at = SystemTime::UNIX_EPOCH;
    assert!(store.get(&key, &store.epoch()).is_none());
    store.remember(key, &store.epoch(), evidence(), SystemTime::UNIX_EPOCH);
    assert!(store.get(&key, &store.epoch()).is_none());
}

#[tokio::test(start_paused = true)]
async fn capacity_evicts_oldest_success_and_reads_do_not_change_eviction_order() {
    let store = MemoryIdentityStore::new(2);
    let first = seed(&store, "first");
    tokio::time::advance(Duration::from_secs(1)).await;
    let second = seed(&store, "second");
    assert!(store.get(&first, &store.epoch()).is_some());
    let third = seed(&store, "third");
    assert!(store.get(&first, &store.epoch()).is_none());
    assert!(store.get(&second, &store.epoch()).is_some());
    assert!(store.get(&third, &store.epoch()).is_some());
    assert_eq!(store.records.lock().unwrap().entries.len(), 2);
}

#[test]
fn rejection_and_logout_fence_all_pending_writes_and_fallback_reads() {
    let store = MemoryIdentityStore::default();
    for clear in [false, true] {
        let key = seed(&store, "fixture-token");
        let pending = store.epoch();
        if clear {
            store.clear();
        } else {
            store.invalidate(&key);
        }
        store.remember(
            key,
            &pending,
            evidence(),
            SystemTime::now() + Duration::from_secs(3600),
        );
        assert!(store.get(&key, &pending).is_none());
        assert!(store.get(&key, &store.epoch()).is_none());
    }
}

#[test]
fn token_hashes_are_keyed_isolated_and_restart_empty() {
    let first = MemoryIdentityStore::default();
    let second = MemoryIdentityStore::default();
    let key = seed(&first, "fixture-token");
    assert_eq!(key.len(), 32);
    assert_eq!(
        key,
        first.key(CredentialPurpose::BrowserCookie, "fixture-token")
    );
    assert_ne!(
        key,
        first.key(CredentialPurpose::BrowserCookie, "other-token")
    );
    assert_ne!(
        key,
        second.key(CredentialPurpose::BrowserCookie, "fixture-token")
    );
    assert!(first
        .get(
            &first.key(CredentialPurpose::BrowserCookie, "other-token"),
            &first.epoch()
        )
        .is_none());
    assert!(second.get(&key, &second.epoch()).is_none());
}

#[test]
fn only_exact_get_session_is_eligible_all_api_and_new_routes_fail_closed() {
    let policy = SessionReadOnly;
    assert!(policy.permits(&Method::GET, "/auth/session"));
    for method in [
        Method::POST,
        Method::DELETE,
        Method::PUT,
        Method::PATCH,
        Method::HEAD,
        Method::OPTIONS,
    ] {
        assert!(!policy.permits(&method, "/auth/session"));
    }
    for path in [
        "/auth/session/",
        "/auth//session",
        "/auth/session/extra",
        "/auth/%73ession",
        "/new-route",
    ] {
        assert!(!policy.permits(&Method::GET, path));
    }
    // This shared inventory is the actual public surface, including sensitive
    // reads and all operations introduced later. No broad GET-prefix allowance.
    let inventory: serde_json::Value = serde_json::from_str(include_str!(
        "../../../layer1-public/tests/fixtures/public-contract-inventory.json"
    ))
    .unwrap();
    for operation in inventory["operations"].as_object().unwrap().keys() {
        let Some((method, path)) = operation.split_once(' ') else {
            continue;
        };
        let Ok(method) = Method::from_bytes(method.as_bytes()) else {
            continue;
        };
        assert_eq!(
            policy.permits(&method, path),
            method == Method::GET && path == "/auth/session",
            "{operation}"
        );
    }
}

#[test]
fn every_fallback_log_is_countable_and_contains_only_safe_hash_and_reason() {
    #[derive(Clone)]
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Writer(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let store = MemoryIdentityStore::default();
    let key = seed(&store, "secret-fixture-token");
    tracing::subscriber::with_default(subscriber, || {
        report(&key, "http_503");
        report(&key, "timeout");
    });
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert_eq!(text.lines().count(), 2);
    for line in text.lines() {
        assert!(line.contains("WARN"));
        assert!(line.contains("event=\"identity_fallback\""));
        assert!(line.contains("count=1"));
        assert!(line.contains("token_hash="));
        assert!(!line.contains("fixture"));
        assert!(!line.contains("subject"));
    }
    assert!(text.contains("http_503"));
    assert!(text.contains("timeout"));
}

#[tokio::test(start_paused = true)]
async fn monotonic_token_expiry_prevents_wall_clock_changes_from_extending_access() {
    let store = MemoryIdentityStore::default();
    let key = store.key(CredentialPurpose::BrowserCookie, "short-lived");
    store.remember(
        key,
        &store.epoch(),
        evidence(),
        SystemTime::now() + Duration::from_secs(3),
    );
    tokio::time::advance(Duration::from_secs(4)).await;
    assert!(store.get(&key, &store.epoch()).is_none());
}

#[test]
fn oversized_evidence_cannot_fill_memory_or_leave_older_grants_available() {
    let store = MemoryIdentityStore::default();
    let key = seed(&store, "fixture-token");
    let mut large = evidence();
    Arc::get_mut(&mut large).unwrap().profile.display_name = Some("x".repeat(MAX_RECORD_BYTES));
    store.remember(
        key,
        &store.epoch(),
        large,
        SystemTime::now() + Duration::from_secs(3600),
    );
    assert!(store.get(&key, &store.epoch()).is_none());
    let mut many = evidence();
    Arc::get_mut(&mut many).unwrap().permissions =
        PermissionEvidence::Active(vec![String::new(); MAX_RECORD_BYTES]);
    store.remember(
        key,
        &store.epoch(),
        many,
        SystemTime::now() + Duration::from_secs(3600),
    );
    assert!(store.get(&key, &store.epoch()).is_none());
}

#[test]
fn identical_token_bytes_cannot_cross_credential_purpose_namespaces() {
    let store = MemoryIdentityStore::default();
    let purposes = [
        CredentialPurpose::BrowserCookie,
        CredentialPurpose::IndependentBearer,
        CredentialPurpose::AgentKey,
        CredentialPurpose::ResourceRequest,
    ];
    let token = "same-credential-bytes";
    for purpose in purposes {
        store.clear();
        let key = store.key(purpose, token);
        store.remember(
            key,
            &store.epoch(),
            evidence(),
            SystemTime::now() + Duration::from_secs(3600),
        );
        let keys: std::collections::HashSet<_> =
            purposes.into_iter().map(|p| store.key(p, token)).collect();
        assert_eq!(keys.len(), 4);
        for candidate in keys {
            assert_eq!(
                store.get(&candidate, &store.epoch()).is_some(),
                candidate == key
            );
        }
        store.invalidate_token(token);
        assert!(store.get(&key, &store.epoch()).is_none());
    }
}
