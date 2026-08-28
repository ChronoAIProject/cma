use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt as _;

use super::*;

fn cipher() -> AuthCipher {
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    AuthCipher::new("test-deployment", &key).unwrap()
}

fn stored_integer(document: &Document, field: &str) -> i64 {
    match document.get(field) {
        Some(Bson::Int32(value)) => i64::from(*value),
        Some(Bson::Int64(value)) => *value,
        other => panic!("expected integer {field}, got {other:?}"),
    }
}

fn at_bson_precision(time: SystemTime) -> SystemTime {
    DateTime::from_system_time(time).to_system_time()
}

async fn explain_find(
    client: &Client,
    database: &str,
    collection: &str,
    filter: Document,
    sort: Document,
    hint: &str,
    limit: usize,
) -> Document {
    let explain = client
        .database(database)
        .run_command(doc! {
            "explain": {
                "find": collection,
                "filter": filter,
                "sort": sort,
                "hint": hint,
                "limit": mongo_limit(limit),
            },
            "verbosity": "executionStats",
        })
        .await
        .unwrap();
    explain.get_document("executionStats").unwrap().clone()
}

#[allow(clippy::too_many_arguments)]
async fn explain_projected_find(
    client: &Client,
    database: &str,
    collection: &str,
    filter: Document,
    projection: Document,
    sort: Document,
    hint: &str,
    limit: usize,
) -> Document {
    let explain = client
        .database(database)
        .run_command(doc! {
            "explain": {
                "find": collection,
                "filter": filter,
                "projection": projection,
                "sort": sort,
                "hint": hint,
                "limit": mongo_limit(limit),
            },
            "verbosity": "executionStats",
        })
        .await
        .unwrap();
    explain.get_document("executionStats").unwrap().clone()
}

fn assert_explain_is_page_bounded(
    execution: &Document,
    returned: usize,
    examined: usize,
    expected_index: &str,
) {
    assert_eq!(
        stored_integer(execution, "nReturned"),
        i64::try_from(returned).unwrap()
    );
    assert!(
        stored_integer(execution, "totalKeysExamined") <= i64::try_from(examined).unwrap(),
        "keys examined exceeded the page bound: {execution:?}"
    );
    assert!(
        stored_integer(execution, "totalDocsExamined") <= i64::try_from(examined).unwrap(),
        "documents examined exceeded the page bound: {execution:?}"
    );
    let mut saw_expected_index = false;
    assert_indexed_execution_stage(
        execution
            .get("executionStages")
            .expect("execution stats contain an execution plan"),
        expected_index,
        &mut saw_expected_index,
    );
    assert!(
        saw_expected_index,
        "execution plan did not use expected index {expected_index}: {execution:?}"
    );
}

fn assert_indexed_execution_stage(value: &Bson, expected_index: &str, saw_expected: &mut bool) {
    match value {
        Bson::Document(stage) => {
            if let Ok(name) = stage.get_str("stage") {
                assert_ne!(
                    name, "COLLSCAN",
                    "execution plan contains COLLSCAN: {stage:?}"
                );
                assert_ne!(
                    name, "SORT",
                    "execution plan contains blocking SORT: {stage:?}"
                );
                if name == "IXSCAN" {
                    assert_eq!(
                        stage.get_str("indexName").ok(),
                        Some(expected_index),
                        "execution plan used an unexpected index: {stage:?}"
                    );
                    *saw_expected = true;
                }
            }
            for child in stage.values() {
                assert_indexed_execution_stage(child, expected_index, saw_expected);
            }
        }
        Bson::Array(children) => {
            for child in children {
                assert_indexed_execution_stage(child, expected_index, saw_expected);
            }
        }
        _ => {}
    }
}

fn subject_state() -> SubjectAuthState {
    SubjectAuthState {
        subject: SubjectId("subject-a".into()),
        incarnation: "subject-incarnation-a".into(),
        revision: 7,
        token_generation: 5,
        tokens: Some(DurableSubjectTokens {
            access_token: Some(SecretValue::new("access-plaintext".into())),
            refresh_token: Some(SecretValue::new("refresh-plaintext".into())),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000),
        }),
        refresh_lease: Some(SubjectRefreshLease {
            holder: "refresh-worker-a".into(),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060),
        }),
        authorization: Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::ReadAllSessions,
        ]),
        authorization_check_after: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_120),
        authorization_lease: None,
        profile: Profile {
            display_name: Some("Example Person".into()),
            email: Some("person@example.test".into()),
            avatar_url: None,
        },
    }
}

fn credential_state() -> SessionCredentialState {
    SessionCredentialState {
        session: SessionId("sesn_a".into()),
        generation: 9,
        request_bound_credential: None,
        current: Some(DurableCredential {
            id: "provider-current".into(),
            value: SecretValue::new("current-plaintext".into()),
            owner: SubjectId("subject-a".into()),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            expires_at: None,
            scope_fingerprint: "scope-a".into(),
            revocation_credential: Some(SecretValue::new("current-revoke-plaintext".into())),
        }),
        pending: Some(DurableCredential {
            id: "provider-pending".into(),
            value: SecretValue::new("pending-plaintext".into()),
            owner: SubjectId("subject-a".into()),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_001),
            expires_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_001)),
            scope_fingerprint: "scope-b".into(),
            revocation_credential: Some(SecretValue::new("pending-revoke-plaintext".into())),
        }),
        pending_target: None,
        revoke_debt: vec![RevokeDebt {
            id: "provider-retired".into(),
            owner: SubjectId("subject-a".into()),
            queued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_002),
            revocation_credential: Some(SecretValue::new("debt-revoke-plaintext".into())),
        }],
        lease: Some(CredentialLease {
            holder: "worker-a".into(),
            fence: 9,
            acquired_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_003),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_063),
            scope_fingerprint: "scope-b".into(),
        }),
        retired: false,
    }
}

#[test]
fn expiry_indexes_are_zero_second_ttl_cleanup_indexes() {
    for (field, name) in [
        ("expires_at", BROWSER_EXPIRY_INDEX),
        ("expires_at", LOGIN_EXPIRY_INDEX),
        ("delete_after", SUBJECT_EXPIRY_INDEX),
        ("delete_after", CREDENTIAL_EXPIRY_INDEX),
    ] {
        let index = expiry_index(field, name);
        let mut expected = Document::new();
        expected.insert(field, 1);
        assert_eq!(index.keys, expected);
        let options = index.options.unwrap();
        assert_eq!(options.name.as_deref(), Some(name));
        assert_eq!(options.expire_after, Some(Duration::ZERO));
    }
}

#[test]
fn recurring_auth_queries_have_named_supporting_indexes() {
    for (keys, name, partial_filter) in [
        (
            doc! { "tokens.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            SUBJECT_REFRESH_INDEX,
            doc! {
                "tokens.refresh_token": { "$type": "object" },
                "refresh_lease": Bson::Null,
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "refresh_lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
            doc! {
                "tokens.refresh_token": { "$type": "object" },
                "refresh_lease.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "sweep_sequence": 1, "_id": 1 },
            SUBJECT_OWNER_INDEX,
            doc! {
                "tokens": { "$type": "object" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "authorization_check_after": 1, "_id": 1 },
            SUBJECT_AUTHORIZATION_INDEX,
            doc! { "tokens": { "$type": "object" } },
        ),
        (
            doc! { "current.issued_at": 1, "sweep_sequence": 1, "_id": 1 },
            CREDENTIAL_CURRENT_INDEX,
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending": Bson::Null,
                "current.issued_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "current.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            CREDENTIAL_CURRENT_EXPIRY_INDEX,
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending": Bson::Null,
                "current.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "sweep_sequence": 1, "_id": 1 },
            CREDENTIAL_PENDING_INDEX,
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending.issued_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            CREDENTIAL_EXPIRED_LEASE_INDEX,
            doc! {
                "retired": false,
                "pending": Bson::Null,
                "current.issued_at": { "$type": "date" },
                "lease.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! { "sweep_sequence": 1, "_id": 1, "revoke_debt.id": 1 },
            CREDENTIAL_DEBT_INDEX,
            doc! {
                "revoke_debt.0": { "$exists": true },
                "revoke_debt_ordered": true,
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            doc! {
                "auth_schema_version": 1,
                "revoke_debt_ordered": 1,
                "_id": 1,
            },
            CREDENTIAL_DEBT_ORDER_INDEX,
            doc! { "revoke_debt.0": { "$exists": true } },
        ),
        (
            doc! { "sweep_sequence": 1, "_id": 1 },
            CREDENTIAL_RECONCILIATION_INDEX,
            doc! {
                "retired": false,
                "sweep_sequence": { "$type": "long" },
            },
        ),
    ] {
        let index = query_index(keys.clone(), name, partial_filter.clone());
        assert_eq!(index.keys, keys);
        let options = index.options.unwrap();
        assert_eq!(options.name.as_deref(), Some(name));
        assert_eq!(options.expire_after, None);
        assert_eq!(options.partial_filter_expression, Some(partial_filter));
    }

    for (keys, name) in [
        (
            doc! { "auth_schema_version": 1, "_id": 1 },
            SUBJECT_MIGRATION_INDEX,
        ),
        (
            doc! { "auth_schema_version": 1, "_id": 1 },
            CREDENTIAL_MIGRATION_INDEX,
        ),
    ] {
        let index = named_index(keys.clone(), name);
        assert_eq!(index.keys, keys);
        let options = index.options.unwrap();
        assert_eq!(options.name.as_deref(), Some(name));
        assert_eq!(options.expire_after, None);
        assert_eq!(options.partial_filter_expression, None);
    }
}

#[test]
fn token_owner_pages_match_the_partial_index_and_use_its_named_hint() {
    let after = SubjectOwnerCursor {
        sweep_sequence: 41,
        subject: SubjectId("subject-m".into()),
    };
    let filter = subject_owner_filter(Some(&after), 99);
    assert_eq!(
        filter
            .get_document("sweep_sequence")
            .unwrap()
            .get_i64("$lte")
            .unwrap(),
        99
    );
    assert_eq!(filter.get_array("$or").unwrap().len(), 2);
    assert_eq!(
        subject_owner_hint(),
        mongodb::options::Hint::Name(SUBJECT_OWNER_INDEX.to_string())
    );
}

#[test]
fn recurring_work_filters_advance_in_their_compound_index_order() {
    let first_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let second_time = first_time + Duration::from_secs(60);
    let subject_cursor = SubjectRefreshCursor {
        ready_at: first_time,
        sweep_sequence: 41,
        subject: SubjectId("subject-m".into()),
    };
    let unleased = unleased_subject_refresh_filter(second_time, Some(&subject_cursor), 99);
    assert_eq!(unleased.get_array("$or").unwrap().len(), 3);
    assert_eq!(
        unleased
            .get_document("tokens.expires_at")
            .unwrap()
            .get_str("$type")
            .unwrap(),
        "date"
    );
    assert_eq!(
        unleased
            .get_document("sweep_sequence")
            .unwrap()
            .get_i64("$lte")
            .unwrap(),
        99
    );
    let expired = expired_subject_refresh_lease_filter(second_time, Some(&subject_cursor), 99);
    assert_eq!(expired.get_array("$or").unwrap().len(), 3);

    let legacy_authorization_cursor = SubjectAuthorizationCursor {
        ready_at: None,
        subject: SubjectId("subject-m".into()),
    };
    let legacy_authorization =
        subject_authorization_filter(second_time, Some(&legacy_authorization_cursor));
    let authorization_base = legacy_authorization.get_array("$and").unwrap()[0]
        .as_document()
        .unwrap();
    let authorization_lease_branches = authorization_base.get_array("$or").unwrap();
    assert_eq!(authorization_lease_branches.len(), 3);
    assert_eq!(
        authorization_lease_branches[2]
            .as_document()
            .unwrap()
            .get_document("authorization_lease.expires_at")
            .unwrap()
            .get_datetime("$lte")
            .unwrap()
            .to_system_time(),
        second_time
    );
    let legacy_position = legacy_authorization.get_array("$and").unwrap()[2]
        .as_document()
        .unwrap()
        .get_array("$or")
        .unwrap();
    assert_eq!(legacy_position.len(), 2);
    assert!(!legacy_position[0]
        .as_document()
        .unwrap()
        .get_document("authorization_check_after")
        .unwrap()
        .get_bool("$exists")
        .unwrap());
    assert_eq!(
        legacy_position[0]
            .as_document()
            .unwrap()
            .get_document("_id")
            .unwrap()
            .get_str("$gt")
            .unwrap(),
        "subject-m"
    );

    let scheduled_authorization_cursor = SubjectAuthorizationCursor {
        ready_at: Some(first_time),
        subject: SubjectId("subject-m".into()),
    };
    let scheduled_authorization =
        subject_authorization_filter(second_time, Some(&scheduled_authorization_cursor));
    let scheduled_position = scheduled_authorization.get_array("$and").unwrap()[2]
        .as_document()
        .unwrap();
    assert_eq!(
        scheduled_position
            .get_document("authorization_check_after")
            .unwrap()
            .get_datetime("$lte")
            .unwrap()
            .to_system_time(),
        second_time
    );
    let scheduled_branches = scheduled_position.get_array("$or").unwrap();
    assert_eq!(scheduled_branches.len(), 2);
    assert_eq!(
        scheduled_branches[0]
            .as_document()
            .unwrap()
            .get_document("authorization_check_after")
            .unwrap()
            .get_datetime("$gt")
            .unwrap()
            .to_system_time(),
        first_time
    );
    assert_eq!(
        scheduled_branches[1]
            .as_document()
            .unwrap()
            .get_datetime("authorization_check_after")
            .unwrap()
            .to_system_time(),
        first_time
    );
    assert_eq!(
        scheduled_branches[1]
            .as_document()
            .unwrap()
            .get_document("_id")
            .unwrap()
            .get_str("$gt")
            .unwrap(),
        "subject-m"
    );

    let credential_cursor = CredentialReissueCursor {
        issued_at: first_time,
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
    };
    let current = current_credential_reissue_filter(second_time, Some(&credential_cursor), 99);
    assert_eq!(current.get_array("$or").unwrap().len(), 3);
    let expiry_cursor = CredentialExpiryCursor {
        expires_at: first_time,
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
    };
    let expiring = current_credential_expiry_filter(second_time, Some(&expiry_cursor), 99);
    assert_eq!(
        expiring
            .get_document("current.expires_at")
            .unwrap()
            .get_datetime("$lte")
            .unwrap()
            .to_system_time(),
        second_time
    );
    assert_eq!(
        expiring
            .get_document("sweep_sequence")
            .unwrap()
            .get_i64("$lte")
            .unwrap(),
        99
    );
    let expiry_branches = expiring.get_array("$or").unwrap();
    assert_eq!(expiry_branches.len(), 3);
    assert_eq!(
        expiry_branches[0]
            .as_document()
            .unwrap()
            .get_document("current.expires_at")
            .unwrap()
            .get_datetime("$gt")
            .unwrap()
            .to_system_time(),
        first_time
    );
    let mutation_cursor = CredentialMutationCursor {
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
    };
    let pending = pending_credential_reissue_filter(Some(&mutation_cursor), 99);
    assert_eq!(pending.get_array("$or").unwrap().len(), 2);
    let reconciliation_cursor = CredentialReconciliationCursor {
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
    };
    let reconciliation = active_credential_reconciliation_filter(Some(&reconciliation_cursor), 99);
    assert_eq!(reconciliation.get_array("$or").unwrap().len(), 2);
    let lease_cursor = CredentialLeaseCursor {
        ready_at: first_time,
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
    };
    let lease = expired_credential_mint_lease_filter(second_time, Some(&lease_cursor), 99);
    assert_eq!(lease.get_array("$or").unwrap().len(), 3);
    let debt_cursor = RevokeDebtCursor {
        sweep_sequence: 42,
        session: SessionId("sesn_m".into()),
        credential_id: "provider-m".into(),
    };
    let parents = credential_debt_parent_filter(Some(&debt_cursor), 99);
    assert_eq!(parents.get_array("$or").unwrap().len(), 2);
    assert_eq!(
        parents
            .get_document("revoke_debt.0.id")
            .unwrap()
            .get_str("$type")
            .unwrap(),
        "string"
    );
    let debt = revoke_debt_after_filter(&debt_cursor, 99);
    assert_eq!(debt.get_str("_id").unwrap(), "sesn_m");
    assert_eq!(
        unordered_revoke_debt_filter(),
        doc! {
            "$or": [
                { "auth_schema_version": { "$exists": false } },
                { "auth_schema_version": { "$in": [1_i64, 2_i64, 3_i64, 4_i64, AUTH_SCHEMA_VERSION] } },
            ],
            "revoke_debt.0": { "$exists": true },
            "revoke_debt_ordered": { "$ne": true },
        }
    );
    assert_eq!(
        revoke_debt_after_projection("provider-m"),
        doc! {
            "_id": 1,
            "auth_schema_version": 1,
            "request_bound_credential": 1,
            "sweep_sequence": 1,
            "revoke_debt": {
                "$elemMatch": { "id": { "$gt": "provider-m" } },
            },
        }
    );
    assert_eq!(
        first_revoke_debt_projection(),
        doc! {
            "_id": 1,
            "auth_schema_version": 1,
            "request_bound_credential": 1,
            "sweep_sequence": 1,
            "revoke_debt": { "$slice": 1 },
        }
    );
    assert_eq!(
        last_revoke_debt_projection(),
        doc! {
            "_id": 1,
            "auth_schema_version": 1,
            "request_bound_credential": 1,
            "sweep_sequence": 1,
            "revoke_debt": { "$slice": -1 },
        }
    );
}

#[test]
fn subject_retention_rolls_with_tokens_and_protects_an_active_lease() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let state = subject_state();
    let token_expiry = state.tokens.as_ref().unwrap().expires_at;
    assert_eq!(
        subject_delete_after(&state, now).unwrap(),
        token_expiry + SUBJECT_RETENTION
    );

    let mut tokenless = subject_state();
    tokenless.tokens = None;
    tokenless.refresh_lease = None;
    assert_eq!(
        subject_delete_after(&tokenless, now).unwrap(),
        now + SUBJECT_RETENTION
    );

    let mut leased = subject_state();
    leased.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH;
    let lease_deadline = now + Duration::from_secs(45);
    leased.refresh_lease.as_mut().unwrap().expires_at = lease_deadline;
    assert_eq!(subject_delete_after(&leased, now).unwrap(), lease_deadline);
}

#[test]
fn only_a_fully_settled_retired_credential_gets_ttl_cleanup() {
    let cipher = cipher();
    let active = credential_state_to_doc(credential_state(), &cipher).unwrap();
    assert_eq!(active.delete_after, None);

    let mut settled = credential_state();
    settled.current = None;
    settled.pending = None;
    settled.revoke_debt.clear();
    settled.lease = None;
    settled.retired = true;
    let earliest = at_bson_precision(SystemTime::now() + SETTLED_CREDENTIAL_RETENTION);
    let stored = credential_state_to_doc(settled.clone(), &cipher).unwrap();
    let latest = at_bson_precision(SystemTime::now() + SETTLED_CREDENTIAL_RETENTION);
    let delete_after = stored.delete_after.unwrap().to_system_time();
    assert!(delete_after >= earliest);
    assert!(delete_after <= latest);

    settled.revoke_debt.push(RevokeDebt {
        id: "late-provider-key".into(),
        owner: SubjectId("subject-a".into()),
        queued_at: SystemTime::now(),
        revocation_credential: None,
    });
    assert_eq!(
        credential_state_to_doc(settled, &cipher)
            .unwrap()
            .delete_after,
        None
    );
}

#[test]
fn subject_secrets_are_encrypted_and_round_trip() {
    let cipher = cipher();
    let mut state = subject_state();
    state.authorization_lease = Some(SubjectAuthorizationLease {
        holder: "authorization-worker-a".into(),
        fence: 7,
        acquired_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100),
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_130),
        checked_incarnation: "subject-incarnation-a".into(),
        checked_token_generation: 5,
    });
    let stored = subject_to_doc(state, &cipher).unwrap();
    let rendered = format!(
        "{:?}",
        mongodb::bson::serialize_to_document(&stored).unwrap()
    );
    assert!(!rendered.contains("access-plaintext"));
    assert!(!rendered.contains("refresh-plaintext"));

    let restored = subject_from_doc(stored, &cipher).unwrap();
    let tokens = restored.tokens.unwrap();
    assert_eq!(
        tokens.access_token.as_ref().unwrap().expose(),
        "access-plaintext"
    );
    assert_eq!(
        tokens.refresh_token.as_ref().unwrap().expose(),
        "refresh-plaintext"
    );
    assert_eq!(restored.revision, 7);
    assert_eq!(restored.token_generation, 5);
    let refresh_lease = restored.refresh_lease.unwrap();
    assert_eq!(refresh_lease.holder, "refresh-worker-a");
    assert_eq!(
        refresh_lease.expires_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060)
    );
    assert!(restored.authorization.allows(Capability::ReadAllSessions));
    assert_eq!(
        restored.authorization_check_after,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_120)
    );
    let authorization_lease = restored.authorization_lease.unwrap();
    assert_eq!(authorization_lease.holder, "authorization-worker-a");
    assert_eq!(authorization_lease.fence, 7);
    assert_eq!(
        authorization_lease.acquired_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100)
    );
    assert_eq!(
        authorization_lease.expires_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_130)
    );
    assert_eq!(
        authorization_lease.checked_incarnation,
        "subject-incarnation-a"
    );
    assert_eq!(authorization_lease.checked_token_generation, 5);
    assert_eq!(
        restored.profile.display_name.as_deref(),
        Some("Example Person")
    );
}

#[test]
fn quarantined_access_is_absent_while_the_encrypted_refresh_grant_round_trips() {
    let cipher = cipher();
    let mut state = subject_state();
    state
        .tokens
        .as_mut()
        .expect("subject fixture has tokens")
        .access_token = None;

    let stored = subject_to_doc(state, &cipher).unwrap();
    let document = mongodb::bson::serialize_to_document(&stored).unwrap();
    let tokens = document.get_document("tokens").unwrap();
    assert!(!tokens.contains_key("access_token"));
    assert!(tokens.contains_key("refresh_token"));

    let restored = subject_from_doc(stored, &cipher).unwrap();
    let tokens = restored.tokens.unwrap();
    assert!(tokens.access_token.is_none());
    assert_eq!(tokens.refresh_token.unwrap().expose(), "refresh-plaintext");
}

#[test]
fn legacy_subject_bson_defaults_incarnation_generation_and_retention() {
    let cipher = cipher();
    let stored = subject_to_doc(subject_state(), &cipher).unwrap();
    let mut document = mongodb::bson::serialize_to_document(&stored).unwrap();
    document.remove("incarnation");
    document.remove("token_generation");
    document.remove("delete_after");
    document.remove("authorization_check_after");
    document.remove("auth_schema_version");

    let legacy: SubjectAuthDoc = mongodb::bson::deserialize_from_document(document).unwrap();
    assert!(legacy.incarnation.is_empty());
    assert_eq!(legacy.token_generation, 0);
    assert_eq!(legacy.delete_after, None);
    assert_eq!(legacy.authorization_check_after, None);
    assert_eq!(legacy.auth_schema_version, 0);
    let restored = subject_from_doc(legacy, &cipher).unwrap();
    assert!(restored.incarnation.is_empty());
    assert_eq!(restored.token_generation, 0);
    assert_eq!(restored.authorization_check_after, SystemTime::UNIX_EPOCH);
    assert_eq!(
        restored.tokens.unwrap().access_token.unwrap().expose(),
        "access-plaintext"
    );
}

#[test]
fn schema_compatibility_accepts_v1_through_v4_and_rejects_future_documents() {
    let cipher = cipher();

    let mut legacy_subject = subject_to_doc(subject_state(), &cipher).unwrap();
    legacy_subject.auth_schema_version = 1;
    assert!(subject_from_doc(legacy_subject, &cipher).is_ok());
    let mut v2_subject = subject_to_doc(subject_state(), &cipher).unwrap();
    v2_subject.auth_schema_version = 2;
    v2_subject.authorization_check_after = None;
    assert!(subject_from_doc(v2_subject, &cipher).is_ok());
    let mut v3_subject = subject_to_doc(subject_state(), &cipher).unwrap();
    v3_subject.auth_schema_version = 3;
    v3_subject.authorization_lease = None;
    assert!(subject_from_doc(v3_subject, &cipher).is_ok());
    let mut v4_subject = subject_to_doc(subject_state(), &cipher).unwrap();
    v4_subject.auth_schema_version = 4;
    assert!(subject_from_doc(v4_subject, &cipher).is_ok());
    let mut future_subject = subject_to_doc(subject_state(), &cipher).unwrap();
    future_subject.auth_schema_version = AUTH_SCHEMA_VERSION + 1;
    assert!(matches!(
        subject_from_doc(future_subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut legacy_credential = credential_state_to_doc(credential_state(), &cipher).unwrap();
    legacy_credential.auth_schema_version = 1;
    assert!(credential_state_from_doc(legacy_credential, &cipher).is_ok());
    let mut v2_credential = credential_state_to_doc(credential_state(), &cipher).unwrap();
    v2_credential.auth_schema_version = 2;
    assert!(credential_state_from_doc(v2_credential, &cipher).is_ok());
    let mut v3_credential = credential_state_to_doc(credential_state(), &cipher).unwrap();
    v3_credential.auth_schema_version = 3;
    assert!(credential_state_from_doc(v3_credential, &cipher).is_ok());
    let mut v4_credential = credential_state_to_doc(credential_state(), &cipher).unwrap();
    v4_credential.auth_schema_version = 4;
    v4_credential.pending_target = None;
    assert!(credential_state_from_doc(v4_credential, &cipher).is_ok());
    let mut future_credential = credential_state_to_doc(credential_state(), &cipher).unwrap();
    future_credential.auth_schema_version = AUTH_SCHEMA_VERSION + 1;
    assert!(matches!(
        credential_state_from_doc(future_credential, &cipher),
        Err(StoreError::Malformed(_))
    ));

    assert_eq!(
        legacy_schema_filter(),
        doc! {
            "$or": [
                { "auth_schema_version": { "$exists": false } },
                { "auth_schema_version": { "$in": [1_i64, 2_i64, 3_i64, 4_i64] } },
            ]
        }
    );
    assert_eq!(
        supported_schema_filter(),
        doc! {
            "$or": [
                { "auth_schema_version": { "$exists": false } },
                { "auth_schema_version": { "$in": [1_i64, 2_i64, 3_i64, 4_i64, AUTH_SCHEMA_VERSION] } },
            ]
        }
    );
}

#[test]
fn moving_subject_ciphertext_to_another_field_fails_closed() {
    let cipher = cipher();
    let mut stored = subject_to_doc(subject_state(), &cipher).unwrap();
    let tokens = stored.tokens.as_mut().unwrap();
    std::mem::swap(&mut tokens.access_token, &mut tokens.refresh_token);
    assert!(matches!(
        subject_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn credential_values_are_encrypted_and_bound_to_current_or_pending() {
    let cipher = cipher();
    let mut state = credential_state();
    state.request_bound_credential = Some(SecretValue::new("request-bound-plaintext".into()));
    let mut stored = credential_state_to_doc(state, &cipher).unwrap();
    let rendered = format!(
        "{:?}",
        mongodb::bson::serialize_to_document(&stored).unwrap()
    );
    assert!(!rendered.contains("current-plaintext"));
    assert!(!rendered.contains("pending-plaintext"));
    assert!(!rendered.contains("request-bound-plaintext"));
    assert!(!rendered.contains("current-revoke-plaintext"));
    assert!(!rendered.contains("pending-revoke-plaintext"));
    assert!(!rendered.contains("debt-revoke-plaintext"));

    let current = stored.current.as_mut().unwrap();
    let pending = stored.pending.as_mut().unwrap();
    std::mem::swap(&mut current.value, &mut pending.value);
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut state = credential_state();
    state.request_bound_credential = Some(SecretValue::new("request-bound-plaintext".into()));
    let mut stored = credential_state_to_doc(state, &cipher).unwrap();
    std::mem::swap(
        stored.request_bound_credential.as_mut().unwrap(),
        &mut stored.current.as_mut().unwrap().value,
    );
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut stored = credential_state_to_doc(credential_state(), &cipher).unwrap();
    let current_revocation = stored
        .current
        .as_mut()
        .unwrap()
        .revocation_credential
        .take()
        .unwrap();
    let pending_revocation = stored
        .pending
        .as_mut()
        .unwrap()
        .revocation_credential
        .take()
        .unwrap();
    stored.current.as_mut().unwrap().revocation_credential = Some(pending_revocation);
    stored.pending.as_mut().unwrap().revocation_credential = Some(current_revocation);
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut stored = credential_state_to_doc(credential_state(), &cipher).unwrap();
    stored.revoke_debt[0].id = "provider-moved".into();
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn projected_revoke_debt_decrypts_its_bound_authority() {
    let cipher = cipher();
    let stored = credential_state_to_doc(credential_state(), &cipher).unwrap();
    let work = revoke_debt_page_work(
        RevokeDebtPageDoc {
            id: stored.id,
            auth_schema_version: stored.auth_schema_version,
            request_bound_credential: stored.request_bound_credential,
            revoke_debt: stored.revoke_debt,
        },
        &cipher,
    )
    .unwrap();

    assert_eq!(work.session.as_str(), "sesn_a");
    assert_eq!(work.debt.id, "provider-retired");
    assert_eq!(
        work.debt
            .revocation_credential
            .expect("projected debt retains revocation authority")
            .expose(),
        "debt-revoke-plaintext"
    );
}

#[test]
fn projected_legacy_revoke_debt_uses_request_bound_authority() {
    let cipher = cipher();
    let mut state = credential_state();
    state.request_bound_credential = Some(SecretValue::new("legacy-request-bound".into()));
    state.revoke_debt[0].revocation_credential = None;
    let stored = credential_state_to_doc(state, &cipher).unwrap();
    let work = revoke_debt_page_work(
        RevokeDebtPageDoc {
            id: stored.id,
            auth_schema_version: 1,
            request_bound_credential: stored.request_bound_credential,
            revoke_debt: stored.revoke_debt,
        },
        &cipher,
    )
    .unwrap();

    assert_eq!(
        work.debt
            .revocation_credential
            .expect("legacy projected debt borrows retained request authority")
            .expose(),
        "legacy-request-bound"
    );
}

#[test]
fn credential_state_round_trips_every_durable_transition_fact() {
    let cipher = cipher();
    let mut state = credential_state();
    state.request_bound_credential = Some(SecretValue::new("request-bound".into()));
    state.pending_target = Some(CredentialDeliveryTarget {
        runtime_generation: 12,
        runtime_id: "runtime-target".into(),
    });
    let restored =
        credential_state_from_doc(credential_state_to_doc(state, &cipher).unwrap(), &cipher)
            .unwrap();
    assert_eq!(restored.session.as_str(), "sesn_a");
    assert_eq!(restored.generation, 9);
    assert_eq!(
        restored.request_bound_credential.unwrap().expose(),
        "request-bound"
    );
    let current = restored.current.unwrap();
    assert_eq!(current.value.expose(), "current-plaintext");
    assert_eq!(
        current.expires_at, None,
        "legacy custody without an expiry remains unbounded"
    );
    assert_eq!(
        current.revocation_credential.unwrap().expose(),
        "current-revoke-plaintext"
    );
    let pending = restored.pending.unwrap();
    assert_eq!(pending.value.expose(), "pending-plaintext");
    assert_eq!(
        pending.expires_at,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_001)),
        "the exact provider expiry survives durable custody round-trip"
    );
    assert_eq!(
        pending.revocation_credential.unwrap().expose(),
        "pending-revoke-plaintext"
    );
    assert_eq!(
        restored.pending_target,
        Some(CredentialDeliveryTarget {
            runtime_generation: 12,
            runtime_id: "runtime-target".into(),
        })
    );
    assert_eq!(restored.revoke_debt[0].id, "provider-retired");
    assert_eq!(
        restored.revoke_debt[0]
            .revocation_credential
            .as_ref()
            .unwrap()
            .expose(),
        "debt-revoke-plaintext"
    );
    assert_eq!(restored.lease.unwrap().fence, 9);
    assert!(!restored.retired);
}

#[test]
fn credential_target_without_pending_custody_is_rejected_on_write_and_read() {
    let cipher = cipher();
    let mut invalid = credential_state();
    invalid.pending = None;
    invalid.pending_target = Some(CredentialDeliveryTarget {
        runtime_generation: 2,
        runtime_id: "runtime-orphan-target".into(),
    });
    assert!(matches!(
        credential_state_to_doc(invalid, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut stored = credential_state_to_doc(credential_state(), &cipher).unwrap();
    stored.pending = None;
    stored.pending_target = Some(CredentialDeliveryTargetDoc {
        runtime_generation: 2,
        runtime_id: "runtime-orphan-target".into(),
    });
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn credential_storage_and_legacy_reads_order_revoke_debt_for_bounded_projection() {
    let cipher = cipher();
    let mut state = credential_state();
    state.revoke_debt = ["provider-z", "provider-a", "provider-m"]
        .into_iter()
        .map(|id| RevokeDebt {
            id: id.into(),
            owner: SubjectId("subject-a".into()),
            queued_at: SystemTime::UNIX_EPOCH,
            revocation_credential: None,
        })
        .collect();
    let mut stored = credential_state_to_doc(state, &cipher).unwrap();
    assert!(stored.revoke_debt_ordered);
    assert_eq!(
        stored
            .revoke_debt
            .iter()
            .map(|debt| debt.id.as_str())
            .collect::<Vec<_>>(),
        ["provider-a", "provider-m", "provider-z"]
    );
    stored.revoke_debt.reverse();
    stored.revoke_debt_ordered = false;
    let restored = credential_state_from_doc(stored, &cipher).unwrap();
    assert_eq!(
        restored
            .revoke_debt
            .iter()
            .map(|debt| debt.id.as_str())
            .collect::<Vec<_>>(),
        ["provider-a", "provider-m", "provider-z"]
    );
}

#[test]
fn negative_stored_counters_and_unknown_capabilities_fail_closed() {
    let cipher = cipher();
    let mut subject = subject_to_doc(subject_state(), &cipher).unwrap();
    subject.revision = -1;
    assert!(matches!(
        subject_from_doc(subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut subject = subject_to_doc(subject_state(), &cipher).unwrap();
    subject.token_generation = -1;
    assert!(matches!(
        subject_from_doc(subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut subject = subject_to_doc(subject_state(), &cipher).unwrap();
    subject.capabilities.push("future-superuser".into());
    assert!(matches!(
        subject_from_doc(subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut credentials = credential_state_to_doc(credential_state(), &cipher).unwrap();
    credentials.generation = -1;
    assert!(matches!(
        credential_state_from_doc(credentials, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn every_capability_round_trips_through_subject_storage() {
    for capability in Capability::ALL {
        assert_eq!(
            super::capability(capability_name(capability)).unwrap(),
            capability
        );
    }
}

#[test]
fn revisions_must_advance_exactly_once_without_overflow() {
    assert!(validate_next(4, 5, "record").is_ok());
    assert!(validate_next(4, 4, "record").is_err());
    assert!(validate_next(4, 6, "record").is_err());
    assert!(validate_next(u64::MAX, u64::MAX, "record").is_err());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_recurring_work_pages_bound_examined_keys_and_documents() {
    const HISTORICAL: usize = 250;
    const CURSOR_POSITION: usize = 199;
    const PAGE_SIZE: usize = 5;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_auth_plan_test_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let now = SystemTime::now();
    let old = now - Duration::from_secs(3_600);
    let future = now + Duration::from_secs(3_600);
    let old_date = DateTime::from_system_time(old);
    let future_date = DateTime::from_system_time(future);

    let mut subjects = Vec::with_capacity(HISTORICAL * 4);
    for position in 0..HISTORICAL {
        subjects.push(doc! {
            "_id": format!("refresh-due-{position:04}"),
            "tokens": {
                "refresh_token": {},
                "expires_at": old_date,
            },
            "refresh_lease": Bson::Null,
        });
        subjects.push(doc! {
            "_id": format!("refresh-future-{position:04}"),
            "tokens": {
                "refresh_token": {},
                "expires_at": future_date,
            },
            "refresh_lease": Bson::Null,
        });
        subjects.push(doc! {
            "_id": format!("refresh-lease-expired-{position:04}"),
            "tokens": {
                "refresh_token": {},
                "expires_at": old_date,
            },
            "refresh_lease": { "expires_at": old_date },
        });
        subjects.push(doc! {
            "_id": format!("refresh-lease-active-{position:04}"),
            "tokens": {
                "refresh_token": {},
                "expires_at": old_date,
            },
            "refresh_lease": { "expires_at": future_date },
        });
    }
    client
        .database(&database_name)
        .collection::<Document>(SUBJECTS)
        .insert_many(subjects)
        .await
        .unwrap();
    client
        .database(&database_name)
        .collection::<Document>(SUBJECTS)
        .update_many(
            Document::new(),
            doc! { "$set": { "sweep_sequence": 1_i64 } },
        )
        .await
        .unwrap();

    let mut credentials = Vec::with_capacity(HISTORICAL * 7 + 2);
    for position in 0..HISTORICAL {
        credentials.push(doc! {
            "_id": format!("current-old-{position:04}"),
            "generation": 0,
            "current": { "issued_at": old_date, "expires_at": old_date },
            "pending": Bson::Null,
            "revoke_debt": [],
            "lease": Bson::Null,
            "retired": false,
        });
        credentials.push(doc! {
            "_id": format!("current-future-{position:04}"),
            "generation": 0,
            "current": { "issued_at": future_date, "expires_at": future_date },
            "pending": Bson::Null,
            "revoke_debt": [],
            "lease": Bson::Null,
            "retired": false,
        });
        credentials.push(doc! {
            "_id": format!("pending-{position:04}"),
            "generation": 0,
            "current": Bson::Null,
            "pending": { "issued_at": old_date },
            "revoke_debt": [],
            "lease": Bson::Null,
            "retired": false,
        });
        credentials.push(doc! {
            "_id": format!("mint-lease-expired-{position:04}"),
            "generation": 0,
            "current": { "issued_at": old_date },
            "pending": Bson::Null,
            "revoke_debt": [],
            "lease": { "expires_at": old_date },
            "retired": false,
        });
        credentials.push(doc! {
            "_id": format!("mint-lease-active-{position:04}"),
            "generation": 0,
            "current": { "issued_at": old_date },
            "pending": Bson::Null,
            "revoke_debt": [],
            "lease": { "expires_at": future_date },
            "retired": false,
        });
        credentials.push(doc! {
            "_id": format!("settled-{position:04}"),
            "generation": 0,
            "current": Bson::Null,
            "pending": Bson::Null,
            "revoke_debt": [],
            "lease": Bson::Null,
            "retired": true,
        });
        credentials.push(doc! {
            "_id": format!("future-unordered-debt-{position:04}"),
            "auth_schema_version": AUTH_SCHEMA_VERSION + 1,
            "generation": 0,
            "current": Bson::Null,
            "pending": Bson::Null,
            "revoke_debt": [{
                "id": format!("future-provider-debt-{position:04}"),
                "owner": "future-debt-owner",
                "queued_at": old_date,
            }],
            "revoke_debt_ordered": false,
            "lease": Bson::Null,
            "retired": true,
        });
    }
    let debts: Vec<_> = (0..HISTORICAL)
        .map(|position| {
            doc! {
                "id": format!("provider-debt-{position:04}"),
                "owner": "debt-owner",
                "queued_at": old_date,
            }
        })
        .collect();
    credentials.push(doc! {
        "_id": "debt-parent",
        "generation": 0,
        "current": Bson::Null,
        "pending": Bson::Null,
        "revoke_debt": debts,
        "revoke_debt_ordered": true,
        "lease": Bson::Null,
        "retired": true,
    });
    let legacy_debts: Vec<_> = ["provider-z", "provider-a", "provider-m"]
        .into_iter()
        .map(|id| {
            doc! {
                "id": id,
                "owner": "legacy-debt-owner",
                "queued_at": old_date,
            }
        })
        .collect();
    credentials.push(doc! {
        "_id": "legacy-debt-parent",
        "generation": 0,
        "current": Bson::Null,
        "pending": Bson::Null,
        "revoke_debt": legacy_debts,
        "lease": Bson::Null,
        "retired": true,
    });
    client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .insert_many(credentials)
        .await
        .unwrap();
    client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .update_many(
            Document::new(),
            doc! { "$set": { "sweep_sequence": 1_i64 } },
        )
        .await
        .unwrap();

    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        legacy_schema_filter(),
        Document::new(),
        SUBJECT_MIGRATION_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(&execution, PAGE_SIZE, PAGE_SIZE, SUBJECT_MIGRATION_INDEX);
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        legacy_schema_filter(),
        Document::new(),
        CREDENTIAL_MIGRATION_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(&execution, PAGE_SIZE, PAGE_SIZE, CREDENTIAL_MIGRATION_INDEX);
    let reconciliation_cursor = CredentialReconciliationCursor {
        sweep_sequence: 1,
        session: SessionId(format!("current-old-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        active_credential_reconciliation_filter(Some(&reconciliation_cursor), u64::MAX),
        doc! { "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_RECONCILIATION_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        CREDENTIAL_RECONCILIATION_INDEX,
    );

    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        subject_authorization_filter(now, None),
        doc! { "authorization_check_after": 1, "_id": 1 },
        SUBJECT_AUTHORIZATION_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        SUBJECT_AUTHORIZATION_INDEX,
    );

    let subject_cursor = SubjectRefreshCursor {
        ready_at: old,
        sweep_sequence: 1,
        subject: SubjectId(format!("refresh-due-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        unleased_subject_refresh_filter(now, Some(&subject_cursor), u64::MAX),
        doc! { "tokens.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
        SUBJECT_REFRESH_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(&execution, PAGE_SIZE, PAGE_SIZE * 2, SUBJECT_REFRESH_INDEX);
    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        unleased_subject_refresh_filter(now, None, u64::MAX),
        doc! { "tokens.expires_at": -1, "sweep_sequence": -1, "_id": -1 },
        SUBJECT_REFRESH_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, SUBJECT_REFRESH_INDEX);

    let expired_cursor = SubjectRefreshCursor {
        ready_at: old,
        sweep_sequence: 1,
        subject: SubjectId(format!("refresh-lease-expired-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        expired_subject_refresh_lease_filter(now, Some(&expired_cursor), u64::MAX),
        doc! { "refresh_lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
        SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
    );
    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        expired_subject_refresh_lease_filter(now, None, u64::MAX),
        doc! { "refresh_lease.expires_at": -1, "sweep_sequence": -1, "_id": -1 },
        SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, SUBJECT_EXPIRED_REFRESH_LEASE_INDEX);

    let current_cursor = CredentialReissueCursor {
        issued_at: old,
        sweep_sequence: 1,
        session: SessionId(format!("current-old-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        unordered_revoke_debt_filter(),
        Document::new(),
        CREDENTIAL_DEBT_ORDER_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, CREDENTIAL_DEBT_ORDER_INDEX);
    store.normalize_one_revoke_debt_order().await.unwrap();
    let normalized_legacy = client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .find_one(doc! { "_id": "legacy-debt-parent" })
        .await
        .unwrap()
        .unwrap();
    assert!(normalized_legacy.get_bool("revoke_debt_ordered").unwrap());
    assert_eq!(
        normalized_legacy
            .get_array("revoke_debt")
            .unwrap()
            .iter()
            .map(|debt| debt.as_document().unwrap().get_str("id").unwrap())
            .collect::<Vec<_>>(),
        ["provider-a", "provider-m", "provider-z"]
    );

    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        current_credential_reissue_filter(now, Some(&current_cursor), u64::MAX),
        doc! { "current.issued_at": 1, "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_CURRENT_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        CREDENTIAL_CURRENT_INDEX,
    );
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        current_credential_reissue_filter(now, None, u64::MAX),
        doc! { "current.issued_at": -1, "sweep_sequence": -1, "_id": -1 },
        CREDENTIAL_CURRENT_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, CREDENTIAL_CURRENT_INDEX);

    let expiry_cursor = CredentialExpiryCursor {
        expires_at: old,
        sweep_sequence: 1,
        session: SessionId(format!("current-old-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        current_credential_expiry_filter(now, Some(&expiry_cursor), u64::MAX),
        doc! { "current.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_CURRENT_EXPIRY_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        CREDENTIAL_CURRENT_EXPIRY_INDEX,
    );
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        current_credential_expiry_filter(now, None, u64::MAX),
        doc! { "current.expires_at": -1, "sweep_sequence": -1, "_id": -1 },
        CREDENTIAL_CURRENT_EXPIRY_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, CREDENTIAL_CURRENT_EXPIRY_INDEX);

    let lease_cursor = CredentialLeaseCursor {
        ready_at: old,
        sweep_sequence: 1,
        session: SessionId(format!("mint-lease-expired-{CURSOR_POSITION:04}")),
    };
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        expired_credential_mint_lease_filter(now, Some(&lease_cursor), u64::MAX),
        doc! { "lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_EXPIRED_LEASE_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        CREDENTIAL_EXPIRED_LEASE_INDEX,
    );
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        expired_credential_mint_lease_filter(now, None, u64::MAX),
        doc! { "lease.expires_at": -1, "sweep_sequence": -1, "_id": -1 },
        CREDENTIAL_EXPIRED_LEASE_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, CREDENTIAL_EXPIRED_LEASE_INDEX);

    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        pending_credential_reissue_filter(
            Some(&CredentialMutationCursor {
                sweep_sequence: 1,
                session: SessionId(format!("pending-{CURSOR_POSITION:04}")),
            }),
            u64::MAX,
        ),
        doc! { "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_PENDING_INDEX,
        PAGE_SIZE,
    )
    .await;
    assert_explain_is_page_bounded(
        &execution,
        PAGE_SIZE,
        PAGE_SIZE * 2,
        CREDENTIAL_PENDING_INDEX,
    );
    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        pending_credential_reissue_filter(None, u64::MAX),
        doc! { "sweep_sequence": -1, "_id": -1 },
        CREDENTIAL_PENDING_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 2, CREDENTIAL_PENDING_INDEX);

    let execution = explain_find(
        &client,
        &database_name,
        CREDENTIALS,
        credential_debt_parent_filter(None, u64::MAX),
        doc! { "sweep_sequence": 1, "_id": 1 },
        CREDENTIAL_DEBT_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 1, CREDENTIAL_DEBT_INDEX);
    let execution = explain_projected_find(
        &client,
        &database_name,
        CREDENTIALS,
        credential_debt_parent_filter(None, u64::MAX),
        last_revoke_debt_projection(),
        doc! { "sweep_sequence": -1, "_id": -1 },
        CREDENTIAL_DEBT_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 1, CREDENTIAL_DEBT_INDEX);
    let projected_debt_cursor = RevokeDebtCursor {
        sweep_sequence: 1,
        session: SessionId("debt-parent".into()),
        credential_id: "provider-debt-0004".into(),
    };
    let execution = explain_projected_find(
        &client,
        &database_name,
        CREDENTIALS,
        revoke_debt_after_filter(&projected_debt_cursor, u64::MAX),
        revoke_debt_after_projection(&projected_debt_cursor.credential_id),
        Document::new(),
        CREDENTIAL_DEBT_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 1, CREDENTIAL_DEBT_INDEX);

    let first_debts = store.revoke_debts(None, u64::MAX, PAGE_SIZE).await.unwrap();
    assert_eq!(first_debts.len(), PAGE_SIZE);
    assert_eq!(
        first_debts[0].state.as_ref().unwrap().debt.id,
        "provider-debt-0000"
    );
    assert_eq!(
        first_debts[PAGE_SIZE - 1].state.as_ref().unwrap().debt.id,
        "provider-debt-0004"
    );
    let debt_cursor = first_debts[PAGE_SIZE - 1].cursor.clone();
    let next_debts = store
        .revoke_debts(Some(&debt_cursor), u64::MAX, PAGE_SIZE)
        .await
        .unwrap();
    assert_eq!(next_debts.len(), PAGE_SIZE);
    assert_eq!(
        next_debts[0].state.as_ref().unwrap().debt.id,
        "provider-debt-0005"
    );
    assert_eq!(
        next_debts[PAGE_SIZE - 1].state.as_ref().unwrap().debt.id,
        "provider-debt-0009"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_schema_maintenance_is_bounded_resumable_and_cutover_safe() {
    // Leave one unversioned row behind the bounded startup batch in addition
    // to the explicit v1 and v2 compatibility fixtures.
    const LEGACY_RECORDS: usize = AUTH_MIGRATION_LANE_BATCH_SIZE + 3;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!(
        "chrono_auth_migration_test_{}",
        uuid::Uuid::new_v4().simple()
    );
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let legacy_cipher = AuthCipher::new("deployment-a", &key).unwrap();

    let mut subjects = Vec::with_capacity(LEGACY_RECORDS);
    let mut credentials = Vec::with_capacity(LEGACY_RECORDS);
    for number in 0..LEGACY_RECORDS {
        let is_v1 = number == LEGACY_RECORDS - 1;
        let is_v2 = number == LEGACY_RECORDS - 2;
        let mut subject = subject_state();
        subject.subject = SubjectId(format!("legacy-subject-{number:03}"));
        subject.revision = 0;
        let mut subject =
            mongodb::bson::serialize_to_document(&subject_to_doc(subject, &legacy_cipher).unwrap())
                .unwrap();
        if is_v1 {
            subject.insert("auth_schema_version", 1_i64);
        } else if is_v2 {
            subject.insert("auth_schema_version", 2_i64);
        } else {
            subject.remove("auth_schema_version");
            subject.remove("incarnation");
        }
        subject.remove("delete_after");
        subject.remove("authorization_check_after");
        subjects.push(subject);

        let mut credential = credential_state();
        credential.session = SessionId(format!("sesn_legacy_{number:03}"));
        credential.generation = 0;
        credential.current = None;
        credential.pending = None;
        credential.revoke_debt.clear();
        credential.lease = None;
        credential.retired = true;
        if is_v1 {
            credential.retired = false;
            credential.request_bound_credential = Some(SecretValue::new("v1-request-bound".into()));
        }
        let mut credential = mongodb::bson::serialize_to_document(
            &credential_state_to_doc(credential, &legacy_cipher).unwrap(),
        )
        .unwrap();
        if is_v1 {
            credential.insert("auth_schema_version", 1_i64);
        } else if is_v2 {
            credential.insert("auth_schema_version", 2_i64);
        } else {
            credential.remove("auth_schema_version");
        }
        credential.remove("delete_after");
        credential.remove("revoke_debt_ordered");
        credentials.push(credential);
    }
    let database = client.database(&database_name);
    database
        .collection::<Document>(SUBJECTS)
        .insert_many(subjects)
        .await
        .unwrap();
    database
        .collection::<Document>(CREDENTIALS)
        .insert_many(credentials)
        .await
        .unwrap();

    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    assert_eq!(
        database
            .collection::<Document>(SUBJECTS)
            .count_documents(doc! { "auth_schema_version": AUTH_SCHEMA_VERSION })
            .await
            .unwrap(),
        u64::try_from(AUTH_MIGRATION_LANE_BATCH_SIZE).unwrap()
    );
    assert_eq!(
        database
            .collection::<Document>(CREDENTIALS)
            .count_documents(doc! { "auth_schema_version": AUTH_SCHEMA_VERSION })
            .await
            .unwrap(),
        u64::try_from(AUTH_MIGRATION_LANE_BATCH_SIZE).unwrap()
    );

    let backlog_subject_id = SubjectId(format!(
        "legacy-subject-{:03}",
        AUTH_MIGRATION_LANE_BATCH_SIZE
    ));
    let due_with_legacy = store
        .subjects_due_for_authorization_check(SystemTime::now(), None, LEGACY_RECORDS)
        .await
        .expect("legacy authorization deadlines are queryable before migration reaches them");
    let legacy_due = due_with_legacy
        .iter()
        .find(|item| item.cursor.subject == backlog_subject_id)
        .expect("missing legacy deadline is immediately due through the named index");
    assert_eq!(legacy_due.cursor.ready_at, None);
    assert_eq!(
        legacy_due.state.as_ref().unwrap().authorization_check_after,
        SystemTime::UNIX_EPOCH
    );
    let mut backlog_subject = store
        .subject(&backlog_subject_id)
        .await
        .expect("read a legacy subject beyond the bounded startup batch")
        .expect("legacy backlog subject remains present");
    assert!(backlog_subject.incarnation.is_empty());
    let backlog_revision = backlog_subject.revision;
    backlog_subject.revision += 1;
    assert!(store
        .replace_subject("", backlog_revision, backlog_subject)
        .await
        .expect("replace a legacy subject before migration reaches it"));
    assert!(!store
        .subject(&backlog_subject_id)
        .await
        .expect("read the upgraded legacy subject")
        .expect("upgraded legacy subject remains present")
        .incarnation
        .is_empty());

    store
        .maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET)
        .await
        .unwrap();
    assert_eq!(
        database
            .collection::<Document>(SUBJECTS)
            .count_documents(doc! { "auth_schema_version": AUTH_SCHEMA_VERSION })
            .await
            .unwrap(),
        u64::try_from(LEGACY_RECORDS).unwrap()
    );
    let v1_session = SessionId(format!("sesn_legacy_{:03}", LEGACY_RECORDS - 1));
    assert_eq!(
        store
            .credential(&v1_session)
            .await
            .expect("v1 credential reads after migration")
            .expect("v1 credential remains")
            .request_bound_credential
            .expect("migration preserves the v1 request-bound field")
            .expose(),
        "v1-request-bound"
    );
    assert_eq!(
        database
            .collection::<Document>(CREDENTIALS)
            .count_documents(doc! { "auth_schema_version": AUTH_SCHEMA_VERSION })
            .await
            .unwrap(),
        u64::try_from(LEGACY_RECORDS).unwrap()
    );

    let mut future_subject = mongodb::bson::serialize_to_document(
        &subject_to_doc(subject_state(), &legacy_cipher).unwrap(),
    )
    .unwrap();
    future_subject.insert("auth_schema_version", AUTH_SCHEMA_VERSION + 1);
    future_subject.insert("future_marker", "subject-must-remain-untouched");
    let mut future_credential = mongodb::bson::serialize_to_document(
        &credential_state_to_doc(credential_state(), &legacy_cipher).unwrap(),
    )
    .unwrap();
    future_credential.insert("auth_schema_version", AUTH_SCHEMA_VERSION + 1);
    future_credential.insert("revoke_debt_ordered", false);
    future_credential.insert("future_marker", "credential-must-remain-untouched");
    let future_subject_id = future_subject.get_str("_id").unwrap().to_string();
    let future_credential_id = future_credential.get_str("_id").unwrap().to_string();
    database
        .collection::<Document>(SUBJECTS)
        .insert_one(future_subject.clone())
        .await
        .unwrap();
    database
        .collection::<Document>(CREDENTIALS)
        .insert_one(future_credential.clone())
        .await
        .unwrap();
    store
        .maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET)
        .await
        .expect("future schemas are ignored rather than downgraded");
    assert_eq!(
        database
            .collection::<Document>(SUBJECTS)
            .find_one(doc! { "_id": &future_subject_id })
            .await
            .unwrap()
            .unwrap(),
        future_subject
    );
    assert_eq!(
        database
            .collection::<Document>(CREDENTIALS)
            .find_one(doc! { "_id": &future_credential_id })
            .await
            .unwrap()
            .unwrap(),
        future_credential
    );
    assert!(matches!(
        store.subject(&SubjectId(future_subject_id)).await,
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        store.credential(&SessionId(future_credential_id)).await,
        Err(StoreError::Malformed(_))
    ));

    database
        .collection::<Document>(SUBJECTS)
        .update_one(
            doc! { "_id": "legacy-subject-000" },
            doc! {
                "$unset": {
                    "auth_schema_version": "",
                    "delete_after": "",
                    "incarnation": "",
                    "token_generation": "",
                    "authorization_check_after": "",
                }
            },
        )
        .await
        .unwrap();
    database
        .collection::<Document>(CREDENTIALS)
        .update_one(
            doc! { "_id": "sesn_legacy_000" },
            doc! {
                "$unset": {
                    "auth_schema_version": "",
                    "delete_after": "",
                    "revoke_debt_ordered": "",
                }
            },
        )
        .await
        .unwrap();
    let stale_subject = database
        .collection::<Document>(SUBJECTS)
        .find_one(doc! { "_id": "legacy-subject-000" })
        .await
        .unwrap()
        .unwrap();
    let stale_subject_revision = stored_integer(&stale_subject, "revision");
    let stale_credential = database
        .collection::<Document>(CREDENTIALS)
        .find_one(doc! { "_id": "sesn_legacy_000" })
        .await
        .unwrap()
        .unwrap();
    let stale_credential_generation = stored_integer(&stale_credential, "generation");
    store
        .maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET)
        .await
        .unwrap();
    let rewritten_subject = database
        .collection::<Document>(SUBJECTS)
        .find_one(doc! { "_id": "legacy-subject-000" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_integer(&rewritten_subject, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert!(rewritten_subject.contains_key("delete_after"));
    assert!(rewritten_subject.contains_key("authorization_check_after"));
    assert!(!rewritten_subject
        .get_str("incarnation")
        .expect("migration restores a subject incarnation")
        .is_empty());
    assert_eq!(
        stored_integer(&rewritten_subject, "token_generation"),
        1,
        "a missing legacy token-generation receives a fresh fence"
    );
    assert_eq!(
        stored_integer(&rewritten_subject, "revision"),
        stale_subject_revision + 1,
        "migration fences a subject writer that pre-read the legacy revision"
    );
    let rewritten_credential = database
        .collection::<Document>(CREDENTIALS)
        .find_one(doc! { "_id": "sesn_legacy_000" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_integer(&rewritten_credential, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert!(rewritten_credential.contains_key("delete_after"));
    assert!(rewritten_credential
        .get_bool("revoke_debt_ordered")
        .unwrap());
    assert_eq!(
        stored_integer(&rewritten_credential, "generation"),
        stale_credential_generation + 1,
        "migration fences a credential writer that pre-read the legacy generation"
    );
    assert_eq!(
        database
            .collection::<Document>(SUBJECTS)
            .replace_one(
                doc! {
                    "_id": "legacy-subject-000",
                    "revision": stale_subject_revision,
                },
                stale_subject,
            )
            .await
            .unwrap()
            .matched_count,
        0,
        "a pre-migration subject replacement cannot erase migrated state"
    );
    assert_eq!(
        database
            .collection::<Document>(CREDENTIALS)
            .replace_one(
                doc! {
                    "_id": "sesn_legacy_000",
                    "generation": stale_credential_generation,
                },
                stale_credential,
            )
            .await
            .unwrap()
            .matched_count,
        0,
        "a pre-migration credential replacement cannot erase migrated state"
    );

    let subjects = database.collection::<Document>(SUBJECTS);
    let credentials = database.collection::<Document>(CREDENTIALS);
    subjects
        .update_one(
            doc! { "_id": "legacy-subject-001" },
            doc! { "$unset": { "auth_schema_version": "" } },
        )
        .await
        .unwrap();
    credentials
        .update_one(
            doc! { "_id": "sesn_legacy_001" },
            doc! { "$unset": { "auth_schema_version": "" } },
        )
        .await
        .unwrap();
    let concurrent_subject = subjects
        .find_one(doc! { "_id": "legacy-subject-001" })
        .await
        .unwrap()
        .unwrap();
    let concurrent_revision = stored_integer(&concurrent_subject, "revision");
    let concurrent_credential = credentials
        .find_one(doc! { "_id": "sesn_legacy_001" })
        .await
        .unwrap()
        .unwrap();
    let concurrent_generation = stored_integer(&concurrent_credential, "generation");
    let (maintenance, subject_write, credential_write) = tokio::join!(
        store.maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET),
        subjects.replace_one(
            doc! {
                "_id": "legacy-subject-001",
                "revision": concurrent_revision,
            },
            concurrent_subject,
        ),
        credentials.replace_one(
            doc! {
                "_id": "sesn_legacy_001",
                "generation": concurrent_generation,
            },
            concurrent_credential,
        ),
    );
    maintenance.unwrap();
    subject_write.unwrap();
    credential_write.unwrap();
    let concurrent_subject = subjects
        .find_one(doc! { "_id": "legacy-subject-001" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_integer(&concurrent_subject, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert!(stored_integer(&concurrent_subject, "revision") > concurrent_revision);
    let concurrent_credential = credentials
        .find_one(doc! { "_id": "sesn_legacy_001" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_integer(&concurrent_credential, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert!(stored_integer(&concurrent_credential, "generation") > concurrent_generation);

    database
        .collection::<Document>(CREDENTIALS)
        .insert_one(doc! {
            "_id": "sesn_malformed_legacy",
            "generation": 0,
            "current": Bson::Null,
            "pending": Bson::Null,
            "revoke_debt": "not-an-array",
            "lease": Bson::Null,
            "retired": true,
        })
        .await
        .unwrap();
    database
        .collection::<Document>(CREDENTIALS)
        .update_one(
            doc! { "_id": "sesn_legacy_001" },
            doc! { "$unset": { "auth_schema_version": "" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET).await,
        Err(StoreError::Unavailable(_))
    ));
    let healthy_after_malformed = database
        .collection::<Document>(CREDENTIALS)
        .find_one(doc! { "_id": "sesn_legacy_001" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_integer(&healthy_after_malformed, "auth_schema_version"),
        AUTH_SCHEMA_VERSION,
        "one malformed migration row does not prevent healthy rows in its batch"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_sequence_epochs_return_exact_positions_and_page_large_debt() {
    const DEBTS: usize = crate::auth::AUTH_SWEEP_BATCH_SIZE + 5;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_auth_cursor_test_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let first_ready = now - Duration::from_secs(120);
    let second_ready = now - Duration::from_secs(60);

    for (id, token_expiry, lease_expiry) in [
        ("subject-unleased-a", first_ready, None),
        ("subject-unleased-z", second_ready, None),
        ("subject-leased-a", first_ready, Some(first_ready)),
        ("subject-leased-z", first_ready, Some(second_ready)),
    ] {
        let mut subject = subject_state();
        subject.subject = SubjectId(id.into());
        subject.revision = 0;
        subject.token_generation = 0;
        subject.tokens.as_mut().unwrap().expires_at = token_expiry;
        subject.refresh_lease = lease_expiry.map(|expires_at| SubjectRefreshLease {
            holder: format!("worker-{id}"),
            expires_at,
        });
        assert!(store.insert_subject(subject).await.unwrap());
    }
    let through_subjects = store.auth_sweep_high_water().await.unwrap();
    let mut interior_subject = subject_state();
    interior_subject.subject = SubjectId("subject-unleased-m".into());
    interior_subject.revision = 0;
    interior_subject.token_generation = 0;
    interior_subject.tokens.as_mut().unwrap().expires_at = first_ready;
    interior_subject.refresh_lease = None;
    assert!(store.insert_subject(interior_subject).await.unwrap());
    let unleased = store
        .unleased_subjects_due_for_refresh(now, None, through_subjects, 10)
        .await
        .unwrap();
    assert_eq!(unleased.len(), 2);
    assert!(unleased
        .iter()
        .all(|item| item.cursor.subject.as_str() != "subject-unleased-m"));
    let unleased = &unleased.last().unwrap().cursor;
    assert_eq!(unleased.subject.as_str(), "subject-unleased-z");
    assert_eq!(unleased.ready_at, at_bson_precision(second_ready));
    let expired_refresh = store
        .subjects_with_expired_refresh_leases(now, None, through_subjects, 10)
        .await
        .unwrap();
    let expired_refresh = &expired_refresh.last().unwrap().cursor;
    assert_eq!(expired_refresh.subject.as_str(), "subject-leased-z");
    assert_eq!(expired_refresh.ready_at, at_bson_precision(second_ready));
    let next_subject_epoch = store.auth_sweep_high_water().await.unwrap();
    assert!(store
        .unleased_subjects_due_for_refresh(now, None, next_subject_epoch, 10)
        .await
        .unwrap()
        .iter()
        .any(|item| item.cursor.subject.as_str() == "subject-unleased-m"));

    let owner = SubjectId("subject-owner".into());
    let held = |id: &str, issued_at| DurableCredential {
        id: id.into(),
        value: SecretValue::new(format!("value-{id}")),
        owner: owner.clone(),
        issued_at,
        expires_at: None,
        scope_fingerprint: "scope".into(),
        revocation_credential: None,
    };
    let mut current_a = credential_state();
    current_a.session = SessionId("sesn_current_a".into());
    current_a.generation = 0;
    current_a.current = Some(held("current-a", first_ready));
    current_a.current.as_mut().unwrap().expires_at = Some(first_ready);
    current_a.pending = None;
    current_a.revoke_debt.clear();
    current_a.lease = None;
    let mut current_z = credential_state();
    current_z.session = SessionId("sesn_current_z".into());
    current_z.generation = 0;
    current_z.current = Some(held("current-z", second_ready));
    current_z.current.as_mut().unwrap().expires_at = Some(second_ready);
    current_z.pending = None;
    current_z.revoke_debt.clear();
    current_z.lease = None;
    let mut pending_a = credential_state();
    pending_a.session = SessionId("sesn_pending_a".into());
    pending_a.generation = 0;
    pending_a.current = None;
    pending_a.pending = Some(held("pending-a", first_ready));
    pending_a.revoke_debt.clear();
    pending_a.lease = None;
    let mut pending_z = credential_state();
    pending_z.session = SessionId("sesn_pending_z".into());
    pending_z.generation = 0;
    pending_z.current = None;
    pending_z.pending = Some(held("pending-z", second_ready));
    pending_z.revoke_debt.clear();
    pending_z.lease = None;
    let mut lease_a = credential_state();
    lease_a.session = SessionId("sesn_lease_a".into());
    lease_a.generation = 0;
    lease_a.current = Some(held("lease-a", first_ready));
    lease_a.pending = None;
    lease_a.revoke_debt.clear();
    lease_a.lease.as_mut().unwrap().expires_at = first_ready;
    let mut lease_z = credential_state();
    lease_z.session = SessionId("sesn_lease_z".into());
    lease_z.generation = 0;
    lease_z.current = Some(held("lease-z", first_ready));
    lease_z.pending = None;
    lease_z.revoke_debt.clear();
    lease_z.lease.as_mut().unwrap().expires_at = second_ready;
    let mut active_z = credential_state();
    active_z.session = SessionId("sesn_z_active".into());
    active_z.generation = 0;
    active_z.current = Some(held("active-z", now + Duration::from_secs(60)));
    active_z.current.as_mut().unwrap().expires_at = Some(now + Duration::from_secs(60));
    active_z.pending = None;
    active_z.revoke_debt.clear();
    active_z.lease = None;
    for state in [
        current_a, current_z, pending_a, pending_z, lease_a, lease_z, active_z,
    ] {
        assert!(store.insert_credential(state).await.unwrap());
    }
    let through_credentials = store.auth_sweep_high_water().await.unwrap();
    let mut interior_current = credential_state();
    interior_current.session = SessionId("sesn_current_m".into());
    interior_current.generation = 0;
    interior_current.current = Some(held("current-m", first_ready));
    interior_current.current.as_mut().unwrap().expires_at = Some(first_ready);
    interior_current.pending = None;
    interior_current.revoke_debt.clear();
    interior_current.lease = None;
    assert!(store.insert_credential(interior_current).await.unwrap());

    let current = store
        .current_credentials_due_for_reissue(now, None, through_credentials, 10)
        .await
        .unwrap();
    assert_eq!(current.len(), 2);
    assert!(current
        .iter()
        .all(|item| item.cursor.session.as_str() != "sesn_current_m"));
    let current = &current.last().unwrap().cursor;
    assert_eq!(current.session.as_str(), "sesn_current_z");
    assert_eq!(current.issued_at, at_bson_precision(second_ready));
    let expiring = store
        .current_credentials_expiring_before(now, None, through_credentials, 10)
        .await
        .unwrap();
    assert_eq!(expiring.len(), 2);
    assert!(expiring
        .iter()
        .all(|item| item.cursor.session.as_str() != "sesn_current_m"));
    let expiring = &expiring.last().unwrap().cursor;
    assert_eq!(expiring.session.as_str(), "sesn_current_z");
    assert_eq!(expiring.expires_at, at_bson_precision(second_ready));
    let pending = store
        .pending_credentials_due_for_reissue(None, through_credentials, 10)
        .await
        .unwrap();
    assert_eq!(
        pending.last().unwrap().cursor.session.as_str(),
        "sesn_pending_z"
    );
    let expired_mint = store
        .credentials_with_expired_mint_leases(now, None, through_credentials, 10)
        .await
        .unwrap();
    let expired_mint = &expired_mint.last().unwrap().cursor;
    assert_eq!(expired_mint.session.as_str(), "sesn_lease_z");
    assert_eq!(expired_mint.ready_at, at_bson_precision(second_ready));
    let active = store
        .active_credentials_for_reconciliation(None, through_credentials, 10)
        .await
        .unwrap();
    assert_eq!(active.last().unwrap().session.as_str(), "sesn_z_active");
    let next_credential_epoch = store.auth_sweep_high_water().await.unwrap();
    assert!(store
        .current_credentials_due_for_reissue(now, None, next_credential_epoch, 10)
        .await
        .unwrap()
        .iter()
        .any(|item| item.cursor.session.as_str() == "sesn_current_m"));
    assert!(store
        .current_credentials_expiring_before(now, None, next_credential_epoch, 10)
        .await
        .unwrap()
        .iter()
        .any(|item| item.cursor.session.as_str() == "sesn_current_m"));

    let debt_session = SessionId("sesn_debt_parent".into());
    let mut debt = credential_state();
    debt.session = debt_session.clone();
    debt.generation = 0;
    debt.current = None;
    debt.pending = None;
    debt.lease = None;
    debt.retired = true;
    debt.revoke_debt = (0..DEBTS)
        .map(|number| RevokeDebt {
            id: format!("provider-debt-{number:03}"),
            owner: owner.clone(),
            queued_at: first_ready,
            revocation_credential: None,
        })
        .collect();
    assert!(store.insert_credential(debt).await.unwrap());
    let through_debt = store.auth_sweep_high_water().await.unwrap();
    let first = store
        .revoke_debts(None, through_debt, crate::auth::AUTH_SWEEP_BATCH_SIZE)
        .await
        .unwrap();
    assert_eq!(first.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
    assert_eq!(
        first.first().unwrap().state.as_ref().unwrap().debt.id,
        "provider-debt-000"
    );
    assert_eq!(
        first.last().unwrap().state.as_ref().unwrap().debt.id,
        "provider-debt-099"
    );
    let cursor = first.last().unwrap().cursor.clone();
    let second = store
        .revoke_debts(
            Some(&cursor),
            through_debt,
            crate::auth::AUTH_SWEEP_BATCH_SIZE,
        )
        .await
        .unwrap();
    assert_eq!(second.len(), 5);
    assert_eq!(
        second.first().unwrap().state.as_ref().unwrap().debt.id,
        "provider-debt-100"
    );
    assert_eq!(
        second.last().unwrap().state.as_ref().unwrap().debt.id,
        "provider-debt-104"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_subject_corruption_isolated_across_recurring_lanes() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("cma_auth_subject_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let now = SystemTime::now();
    let ready_at = now - Duration::from_secs(60);

    for (lane, leased) in [("unleased", false), ("leased", true)] {
        for kind in ["malformed", "ciphertext", "healthy"] {
            let id = format!("subject-{lane}-{kind}");
            let mut state = subject_state();
            state.subject = SubjectId(id.clone());
            state.revision = 0;
            state.token_generation = 0;
            state.tokens.as_mut().unwrap().expires_at = ready_at;
            state.refresh_lease = leased.then(|| SubjectRefreshLease {
                holder: format!("worker-{id}"),
                expires_at: ready_at,
            });
            assert!(store.insert_subject(state).await.unwrap());
        }
    }

    let subjects = client
        .database(&database_name)
        .collection::<Document>(SUBJECTS);
    for lane in ["unleased", "leased"] {
        subjects
            .update_one(
                doc! { "_id": format!("subject-{lane}-malformed") },
                doc! { "$set": { "revision": "not-a-counter" } },
            )
            .await
            .unwrap();
        subjects
            .update_one(
                doc! { "_id": format!("subject-{lane}-ciphertext") },
                doc! { "$set": { "tokens.access_token.version": 999_i32 } },
            )
            .await
            .unwrap();
    }

    let through = store.auth_sweep_high_water().await.unwrap();
    let unleased = store
        .unleased_subjects_due_for_refresh(now, None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        unleased
            .iter()
            .map(|item| item.cursor.subject.as_str())
            .collect::<Vec<_>>(),
        [
            "subject-unleased-malformed",
            "subject-unleased-ciphertext",
            "subject-unleased-healthy",
        ]
    );
    assert!(unleased[0].state.is_err());
    assert!(unleased[1].state.is_err());
    assert_eq!(
        unleased[2].state.as_ref().unwrap().subject.as_str(),
        "subject-unleased-healthy",
        "later healthy unleased refresh work survives both corrupt predecessors"
    );

    let leased = store
        .subjects_with_expired_refresh_leases(now, None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        leased
            .iter()
            .map(|item| item.cursor.subject.as_str())
            .collect::<Vec<_>>(),
        [
            "subject-leased-malformed",
            "subject-leased-ciphertext",
            "subject-leased-healthy",
        ]
    );
    assert!(leased[0].state.is_err());
    assert!(leased[1].state.is_err());
    assert_eq!(
        leased[2].state.as_ref().unwrap().subject.as_str(),
        "subject-leased-healthy",
        "later healthy expired-lease refresh work survives both corrupt predecessors"
    );

    let owners = store.subjects_with_tokens(None, through, 10).await.unwrap();
    let owner_ids = owners
        .iter()
        .map(|cursor| cursor.subject.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        &owner_ids[..3],
        [
            "subject-unleased-malformed",
            "subject-unleased-ciphertext",
            "subject-unleased-healthy",
        ],
        "provider reconciliation publishes independent point-read positions"
    );
    assert!(store
        .subject(&SubjectId(owner_ids[0].to_string()))
        .await
        .is_err());
    assert!(store
        .subject(&SubjectId(owner_ids[1].to_string()))
        .await
        .is_err());
    assert_eq!(
        store
            .subject(&SubjectId(owner_ids[2].to_string()))
            .await
            .unwrap()
            .unwrap()
            .subject
            .as_str(),
        "subject-unleased-healthy",
        "a healthy owner after corrupt point reads remains readable"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_credential_corruption_isolated_across_recurring_lanes() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("cma_auth_credential_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let now = SystemTime::now();
    let ready_at = now - Duration::from_secs(60);
    let future = now + Duration::from_secs(60);

    for lane in ["current", "expiring", "pending", "lease", "active"] {
        for kind in ["malformed", "ciphertext", "healthy"] {
            let id = format!("sesn-{lane}-{kind}");
            let mut state = credential_state();
            state.session = SessionId(id);
            state.generation = 0;
            state.revoke_debt.clear();
            state.retired = false;
            match lane {
                "current" => {
                    state.current.as_mut().unwrap().issued_at = ready_at;
                    state.pending = None;
                    state.lease = None;
                }
                "expiring" => {
                    state.current.as_mut().unwrap().issued_at = future;
                    state.current.as_mut().unwrap().expires_at = Some(ready_at);
                    state.pending = None;
                    state.lease = None;
                }
                "pending" => {
                    state.current = None;
                    state.pending.as_mut().unwrap().issued_at = ready_at;
                    state.lease = None;
                }
                "lease" => {
                    state.current.as_mut().unwrap().issued_at = ready_at;
                    state.pending = None;
                    state.lease.as_mut().unwrap().expires_at = ready_at;
                }
                "active" => {
                    state.current.as_mut().unwrap().issued_at = future;
                    state.pending = None;
                    state.lease = None;
                }
                _ => unreachable!(),
            }
            assert!(store.insert_credential(state).await.unwrap());
        }
    }

    let credentials = client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS);
    for lane in ["current", "expiring", "pending", "lease", "active"] {
        credentials
            .update_one(
                doc! { "_id": format!("sesn-{lane}-malformed") },
                doc! { "$set": { "generation": "not-a-counter" } },
            )
            .await
            .unwrap();
        let value_field = if lane == "pending" {
            "pending.value.version"
        } else {
            "current.value.version"
        };
        credentials
            .update_one(
                doc! { "_id": format!("sesn-{lane}-ciphertext") },
                doc! { "$set": { value_field: 999_i32 } },
            )
            .await
            .unwrap();
    }

    let through = store.auth_sweep_high_water().await.unwrap();
    let current = store
        .current_credentials_due_for_reissue(now, None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        current
            .iter()
            .map(|item| item.cursor.session.as_str())
            .collect::<Vec<_>>(),
        [
            "sesn-current-malformed",
            "sesn-current-ciphertext",
            "sesn-current-healthy",
        ]
    );
    assert!(current[0].state.is_err());
    assert!(current[1].state.is_err());
    assert_eq!(
        current[2].state.as_ref().unwrap().session.as_str(),
        "sesn-current-healthy",
        "later healthy current-credential work survives both corrupt predecessors"
    );

    let expiring = store
        .current_credentials_expiring_before(now, None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        expiring
            .iter()
            .map(|item| item.cursor.session.as_str())
            .collect::<Vec<_>>(),
        [
            "sesn-expiring-malformed",
            "sesn-expiring-ciphertext",
            "sesn-expiring-healthy",
        ]
    );
    assert!(expiring[0].state.is_err());
    assert!(expiring[1].state.is_err());
    assert_eq!(
        expiring[2].state.as_ref().unwrap().session.as_str(),
        "sesn-expiring-healthy",
        "later healthy expiry-ordered work survives both corrupt predecessors"
    );

    let pending = store
        .pending_credentials_due_for_reissue(None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|item| item.cursor.session.as_str())
            .collect::<Vec<_>>(),
        [
            "sesn-pending-malformed",
            "sesn-pending-ciphertext",
            "sesn-pending-healthy",
        ]
    );
    assert!(pending[0].state.is_err());
    assert!(pending[1].state.is_err());
    assert_eq!(
        pending[2].state.as_ref().unwrap().session.as_str(),
        "sesn-pending-healthy",
        "later healthy pending-credential work survives both corrupt predecessors"
    );

    let leases = store
        .credentials_with_expired_mint_leases(now, None, through, 10)
        .await
        .unwrap();
    assert_eq!(
        leases
            .iter()
            .map(|item| item.cursor.session.as_str())
            .collect::<Vec<_>>(),
        [
            "sesn-lease-malformed",
            "sesn-lease-ciphertext",
            "sesn-lease-healthy",
        ]
    );
    assert!(leases[0].state.is_err());
    assert!(leases[1].state.is_err());
    assert_eq!(
        leases[2].state.as_ref().unwrap().session.as_str(),
        "sesn-lease-healthy",
        "later healthy expired-mint-lease work survives both corrupt predecessors"
    );

    let active = store
        .active_credentials_for_reconciliation(None, through, 20)
        .await
        .unwrap();
    let active_ids = active
        .iter()
        .map(|cursor| cursor.session.as_str())
        .filter(|id| id.starts_with("sesn-active-"))
        .collect::<Vec<_>>();
    assert_eq!(
        active_ids,
        [
            "sesn-active-malformed",
            "sesn-active-ciphertext",
            "sesn-active-healthy",
        ],
        "active reconciliation publishes independent point-read positions"
    );
    assert!(store
        .credential(&SessionId(active_ids[0].to_string()))
        .await
        .is_err());
    assert!(store
        .credential(&SessionId(active_ids[1].to_string()))
        .await
        .is_err());
    assert_eq!(
        store
            .credential(&SessionId(active_ids[2].to_string()))
            .await
            .unwrap()
            .unwrap()
            .session
            .as_str(),
        "sesn-active-healthy",
        "healthy custody after corrupt point reads remains readable"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_malformed_revoke_debt_does_not_block_later_healthy_rows() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!(
        "chrono_auth_corrupt_debt_test_{}",
        uuid::Uuid::new_v4().simple()
    );
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let owner = SubjectId("owner".into());
    for number in 0..3 {
        let mut state = credential_state();
        state.session = SessionId(format!("sesn_debt_{number}"));
        state.generation = 0;
        state.current = None;
        state.pending = None;
        state.lease = None;
        state.retired = true;
        state.revoke_debt = vec![RevokeDebt {
            id: format!("provider-debt-{number}"),
            owner: owner.clone(),
            queued_at: SystemTime::now(),
            revocation_credential: None,
        }];
        assert!(store.insert_credential(state).await.unwrap());
    }
    client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .update_one(
            doc! { "_id": "sesn_debt_1" },
            doc! { "$set": { "revoke_debt.0.owner": 42 } },
        )
        .await
        .unwrap();

    let through = store.auth_sweep_high_water().await.unwrap();
    let page = store.revoke_debts(None, through, 10).await.unwrap();
    assert_eq!(page.len(), 3);
    assert_eq!(page[0].state.as_ref().unwrap().debt.id, "provider-debt-0");
    assert!(matches!(page[1].state, Err(StoreError::Malformed(_))));
    assert_eq!(
        page[2].state.as_ref().unwrap().debt.id,
        "provider-debt-2",
        "a malformed projected body advances by its independently decoded cursor"
    );

    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_has_exact_indexes_and_cross_instance_atomicity() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_auth_test_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let legacy_cipher = AuthCipher::new("deployment-a", &key).unwrap();
    let mut legacy_subject = subject_state();
    legacy_subject.subject = SubjectId("legacy-subject".into());
    legacy_subject.revision = 0;
    let expected_subject_delete_after =
        at_bson_precision(legacy_subject.tokens.as_ref().unwrap().expires_at + SUBJECT_RETENTION);
    let mut legacy_subject = mongodb::bson::serialize_to_document(
        &subject_to_doc(legacy_subject, &legacy_cipher).unwrap(),
    )
    .unwrap();
    legacy_subject.remove("incarnation");
    legacy_subject.remove("token_generation");
    legacy_subject.remove("delete_after");
    legacy_subject.remove("authorization_check_after");
    legacy_subject.remove("auth_schema_version");
    client
        .database(&database_name)
        .collection::<Document>(SUBJECTS)
        .insert_one(legacy_subject)
        .await
        .unwrap();

    let mut legacy_credential = credential_state();
    legacy_credential.session = SessionId("sesn_legacy_settled".into());
    legacy_credential.current = None;
    legacy_credential.pending = None;
    legacy_credential.revoke_debt.clear();
    legacy_credential.lease = None;
    legacy_credential.retired = true;
    legacy_credential.request_bound_credential =
        Some(SecretValue::new("legacy-terminal-only".into()));
    let mut legacy_credential = mongodb::bson::serialize_to_document(
        &credential_state_to_doc(legacy_credential, &legacy_cipher).unwrap(),
    )
    .unwrap();
    legacy_credential.remove("delete_after");
    legacy_credential.remove("auth_schema_version");
    client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .insert_one(legacy_credential)
        .await
        .unwrap();
    client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .create_index(query_index(
            doc! { "revoke_debt_ordered": 1, "_id": 1 },
            LEGACY_CREDENTIAL_DEBT_ORDER_INDEX,
            doc! { "revoke_debt.0": { "$exists": true } },
        ))
        .await
        .unwrap();

    let credential_backfill_started =
        at_bson_precision(SystemTime::now() + SETTLED_CREDENTIAL_RETENTION);
    let first = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let credential_backfill_finished =
        at_bson_precision(SystemTime::now() + SETTLED_CREDENTIAL_RETENTION);
    let second = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();

    let raw_legacy_subject = client
        .database(&database_name)
        .collection::<Document>(SUBJECTS)
        .find_one(doc! { "_id": "legacy-subject" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        raw_legacy_subject
            .get_datetime("delete_after")
            .unwrap()
            .to_system_time(),
        expected_subject_delete_after
    );
    assert_eq!(
        stored_integer(&raw_legacy_subject, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert_eq!(stored_integer(&raw_legacy_subject, "revision"), 1);
    assert_eq!(stored_integer(&raw_legacy_subject, "token_generation"), 1);
    assert!(raw_legacy_subject.contains_key("authorization_check_after"));
    assert!(!raw_legacy_subject
        .get_str("incarnation")
        .expect("migration supplies an incarnation")
        .is_empty());
    let restored_legacy_subject = first
        .subject(&SubjectId("legacy-subject".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(!restored_legacy_subject.incarnation.is_empty());
    assert_eq!(restored_legacy_subject.revision, 1);
    assert_eq!(restored_legacy_subject.token_generation, 1);
    assert!(restored_legacy_subject.authorization_check_after <= SystemTime::now());
    let raw_legacy_credential = client
        .database(&database_name)
        .collection::<Document>(CREDENTIALS)
        .find_one(doc! { "_id": "sesn_legacy_settled" })
        .await
        .unwrap()
        .unwrap();
    let credential_delete_after = raw_legacy_credential
        .get_datetime("delete_after")
        .unwrap()
        .to_system_time();
    assert!(credential_delete_after >= credential_backfill_started);
    assert!(credential_delete_after <= credential_backfill_finished);
    assert!(raw_legacy_credential
        .get_bool("revoke_debt_ordered")
        .unwrap());
    assert!(!raw_legacy_credential.contains_key("request_bound_credential"));
    assert_eq!(
        stored_integer(&raw_legacy_credential, "auth_schema_version"),
        AUTH_SCHEMA_VERSION
    );
    assert_eq!(stored_integer(&raw_legacy_credential, "generation"), 10);

    for (collection, expected_name, expected_field) in [
        (BROWSER_SESSIONS, BROWSER_EXPIRY_INDEX, "expires_at"),
        (LOGIN_ATTEMPTS, LOGIN_EXPIRY_INDEX, "expires_at"),
        (SUBJECTS, SUBJECT_EXPIRY_INDEX, "delete_after"),
        (CREDENTIALS, CREDENTIAL_EXPIRY_INDEX, "delete_after"),
    ] {
        let indexes = client
            .database(&database_name)
            .collection::<mongodb::bson::Document>(collection)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let expiry = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(expected_name)
            })
            .expect("named expiry index exists");
        let mut expected_keys = Document::new();
        expected_keys.insert(expected_field, 1);
        assert_eq!(expiry.keys, expected_keys);
        assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));
    }

    for (collection, expected_name, expected_keys, expected_partial) in [
        (
            SUBJECTS,
            SUBJECT_REFRESH_INDEX,
            doc! { "tokens.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            doc! {
                "tokens.refresh_token": { "$type": "object" },
                "refresh_lease": Bson::Null,
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            SUBJECTS,
            SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
            doc! { "refresh_lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            doc! {
                "tokens.refresh_token": { "$type": "object" },
                "refresh_lease.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            SUBJECTS,
            SUBJECT_OWNER_INDEX,
            doc! { "sweep_sequence": 1, "_id": 1 },
            doc! {
                "tokens": { "$type": "object" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            SUBJECTS,
            SUBJECT_AUTHORIZATION_INDEX,
            doc! { "authorization_check_after": 1, "_id": 1 },
            doc! { "tokens": { "$type": "object" } },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_CURRENT_INDEX,
            doc! { "current.issued_at": 1, "sweep_sequence": 1, "_id": 1 },
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending": Bson::Null,
                "current.issued_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_CURRENT_EXPIRY_INDEX,
            doc! { "current.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending": Bson::Null,
                "current.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_PENDING_INDEX,
            doc! { "sweep_sequence": 1, "_id": 1 },
            doc! {
                "retired": false,
                "lease": Bson::Null,
                "pending.issued_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_EXPIRED_LEASE_INDEX,
            doc! { "lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
            doc! {
                "retired": false,
                "pending": Bson::Null,
                "current.issued_at": { "$type": "date" },
                "lease.expires_at": { "$type": "date" },
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_DEBT_INDEX,
            doc! { "sweep_sequence": 1, "_id": 1, "revoke_debt.id": 1 },
            doc! {
                "revoke_debt.0": { "$exists": true },
                "revoke_debt_ordered": true,
                "sweep_sequence": { "$type": "long" },
            },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_DEBT_ORDER_INDEX,
            doc! {
                "auth_schema_version": 1,
                "revoke_debt_ordered": 1,
                "_id": 1,
            },
            doc! { "revoke_debt.0": { "$exists": true } },
        ),
        (
            CREDENTIALS,
            CREDENTIAL_RECONCILIATION_INDEX,
            doc! { "sweep_sequence": 1, "_id": 1 },
            doc! {
                "retired": false,
                "sweep_sequence": { "$type": "long" },
            },
        ),
    ] {
        let indexes = client
            .database(&database_name)
            .collection::<Document>(collection)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let index = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(expected_name)
            })
            .expect("named authentication query index exists");
        assert_eq!(index.keys, expected_keys);
        let options = index.options.unwrap();
        assert_eq!(options.expire_after, None);
        assert_eq!(options.partial_filter_expression, Some(expected_partial));
    }

    for (collection, expected_name) in [
        (SUBJECTS, SUBJECT_MIGRATION_INDEX),
        (CREDENTIALS, CREDENTIAL_MIGRATION_INDEX),
    ] {
        let indexes = client
            .database(&database_name)
            .collection::<Document>(collection)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let index = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(expected_name)
            })
            .expect("named authentication migration index exists");
        assert_eq!(index.keys, doc! { "auth_schema_version": 1, "_id": 1 });
        let options = index.options.unwrap();
        assert_eq!(options.expire_after, None);
        assert_eq!(options.partial_filter_expression, None);
    }

    for id in ["aaa-tokenless", "bbb-tokenless"] {
        let mut tokenless = subject_state();
        tokenless.subject = SubjectId(id.into());
        tokenless.revision = 0;
        tokenless.token_generation = 0;
        tokenless.tokens = None;
        tokenless.refresh_lease = None;
        assert!(first.insert_subject(tokenless).await.unwrap());
    }
    let through_sequence = first.auth_sweep_high_water().await.unwrap();
    let token_owners = first
        .subjects_with_tokens(None, through_sequence, 1)
        .await
        .unwrap();
    assert_eq!(token_owners.len(), 1);
    assert_eq!(token_owners[0].subject.as_str(), "legacy-subject");

    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        subject_owner_filter(None, through_sequence),
        doc! { "sweep_sequence": 1, "_id": 1 },
        SUBJECT_OWNER_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 1, SUBJECT_OWNER_INDEX);
    let execution = explain_find(
        &client,
        &database_name,
        SUBJECTS,
        subject_owner_filter(None, through_sequence),
        doc! { "sweep_sequence": -1, "_id": -1 },
        SUBJECT_OWNER_INDEX,
        1,
    )
    .await;
    assert_explain_is_page_bounded(&execution, 1, 1, SUBJECT_OWNER_INDEX);
    assert_eq!(token_owners[0].subject.as_str(), "legacy-subject");

    let browser_id = SecretDigest::identify("browser-session", "browser-cookie");
    first
        .put_browser_session(BrowserSession {
            id: browser_id.clone(),
            subject: SubjectId("subject-a".into()),
            expires_at: SystemTime::now() + Duration::from_secs(600),
        })
        .await
        .unwrap();
    assert_eq!(
        second
            .browser_session(&browser_id)
            .await
            .unwrap()
            .unwrap()
            .subject
            .as_str(),
        "subject-a"
    );

    let login_id = SecretDigest::identify("login-attempt", "oauth-state");
    first
        .put_login_attempt(LoginAttempt {
            id: login_id.clone(),
            verifier: SecretValue::new("pkce-plaintext".into()),
            expires_at: SystemTime::now() + Duration::from_secs(300),
        })
        .await
        .unwrap();
    let raw_login = client
        .database(&database_name)
        .collection::<mongodb::bson::Document>(LOGIN_ATTEMPTS)
        .find_one(doc! { "_id": id_binary(&login_id) })
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw_login:?}").contains("pkce-plaintext"));

    let (left, right) = tokio::join!(
        first.take_login_attempt(&login_id),
        second.take_login_attempt(&login_id)
    );
    let consumed = [left.unwrap(), right.unwrap()];
    assert_eq!(consumed.iter().filter(|value| value.is_some()).count(), 1);
    assert_eq!(
        consumed
            .into_iter()
            .flatten()
            .next()
            .unwrap()
            .verifier
            .expose(),
        "pkce-plaintext"
    );

    let mut initial = subject_state();
    initial.revision = 0;
    assert!(first.insert_subject(initial).await.unwrap());
    let mut changed_by_first = subject_state();
    changed_by_first.revision = 1;
    changed_by_first.profile.display_name = Some("first".into());
    let mut changed_by_second = subject_state();
    changed_by_second.revision = 1;
    changed_by_second.profile.display_name = Some("second".into());
    let (left, right) = tokio::join!(
        first.replace_subject("subject-incarnation-a", 0, changed_by_first),
        second.replace_subject("subject-incarnation-a", 0, changed_by_second)
    );
    assert_eq!(
        [left.unwrap(), right.unwrap()]
            .into_iter()
            .filter(|won| *won)
            .count(),
        1
    );

    let aba_subject = SubjectId("subject-ttl-recreation-aba".into());
    let mut lifecycle_a = subject_state();
    lifecycle_a.subject = aba_subject.clone();
    lifecycle_a.incarnation = "ttl-lifecycle-a".into();
    lifecycle_a.revision = 0;
    lifecycle_a.profile.display_name = Some("lifecycle-a".into());
    assert!(first.insert_subject(lifecycle_a).await.unwrap());
    let mut stale_lifecycle_a = first
        .subject(&aba_subject)
        .await
        .unwrap()
        .expect("lifecycle A exists before TTL deletion");
    assert_eq!(
        client
            .database(&database_name)
            .collection::<Document>(SUBJECTS)
            .delete_one(doc! { "_id": aba_subject.as_str() })
            .await
            .unwrap()
            .deleted_count,
        1
    );

    let mut lifecycle_b = subject_state();
    lifecycle_b.subject = aba_subject.clone();
    lifecycle_b.incarnation = "ttl-lifecycle-b".into();
    lifecycle_b.revision = 0;
    lifecycle_b.profile.display_name = Some("lifecycle-b".into());
    assert!(second.insert_subject(lifecycle_b).await.unwrap());

    stale_lifecycle_a.revision = 1;
    stale_lifecycle_a.profile.display_name = Some("stale-lifecycle-a-write".into());
    assert!(!first
        .replace_subject("ttl-lifecycle-a", 0, stale_lifecycle_a)
        .await
        .unwrap());
    let preserved_lifecycle_b = second
        .subject(&aba_subject)
        .await
        .unwrap()
        .expect("lifecycle B survives the stale lifecycle-A CAS");
    assert_eq!(preserved_lifecycle_b.incarnation, "ttl-lifecycle-b");
    assert_eq!(preserved_lifecycle_b.revision, 0);
    assert_eq!(
        preserved_lifecycle_b.profile.display_name.as_deref(),
        Some("lifecycle-b")
    );

    drop(first);
    drop(second);
    let restarted = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    assert!(restarted
        .browser_session(&browser_id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        restarted
            .subject(&SubjectId("subject-a".into()))
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );

    client.database(&database_name).drop().await.unwrap();
}
