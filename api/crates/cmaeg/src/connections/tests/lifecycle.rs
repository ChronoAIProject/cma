use super::*;

#[tokio::test]
async fn credentials_are_encrypted_in_backend_and_absent_from_public_views() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let ciphertext = STANDARD
        .decode(&fixture.peer.rows.lock().unwrap()[&record.id].blob)
        .unwrap();
    let requests = serde_json::to_string(&*fixture.peer.requests.lock().unwrap()).unwrap();
    let service = fixture.service();
    let view = service
        .get(&owner("alice", false), &record.id)
        .await
        .unwrap();
    let public = serde_json::to_string(&view).unwrap();
    let debug = format!("{view:?}");
    for secret in [
        CHILD_TOKEN,
        WEBHOOK_SECRET,
        REPLY_TOKEN,
        &record.setup_nonce,
        record.browser_nonce.as_deref().unwrap(),
    ] {
        assert!(!ciphertext
            .windows(secret.len())
            .any(|part| part == secret.as_bytes()));
        assert!(!requests.contains(secret));
        assert!(!public.contains(secret));
        assert!(!debug.contains(secret));
    }
    assert!(view
        .setup_url
        .as_deref()
        .unwrap()
        .starts_with("https://gateway.example/connect/start/cmaeg1.cs."));
    assert!(public.contains("Paper assistant"));
    assert!(!public.contains("origin"));
    assert!(!public.contains("material"));
    let (restored, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(restored.material.unwrap().token, CHILD_TOKEN);
}

#[tokio::test]
async fn ciphertext_cannot_be_moved_to_another_record_or_namespace() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let original = fixture.peer.rows.lock().unwrap()[&record.id].clone();
    let ciphertext = STANDARD.decode(&original.blob).unwrap();
    let target = uuid::Uuid::new_v4().to_string();
    fixture
        .peer
        .rows
        .lock()
        .unwrap()
        .insert(target.clone(), original.clone());
    assert!(matches!(
        fixture.store().connection(&target).await,
        Err(Error::Unavailable)
    ));
    assert!(open_connection(&keys(20), &record.id, &ciphertext).is_err());
    assert!(open_blob(&fixture.keys, &ciphertext).is_err());
    assert!(open_interaction(&fixture.keys, &record.id, &ciphertext).is_err());
    let serialized = serde_json::to_vec(&Stored::Connection(Box::new(record.clone()))).unwrap();
    for wrong_namespace in [
        seal_blob(&fixture.keys, &serialized).unwrap(),
        seal_interaction(&fixture.keys, &record.id, &serialized).unwrap(),
    ] {
        fixture
            .peer
            .rows
            .lock()
            .unwrap()
            .get_mut(&record.id)
            .unwrap()
            .blob = STANDARD.encode(wrong_namespace);
        assert!(matches!(
            fixture.store().connection(&record.id).await,
            Err(Error::Unavailable)
        ));
    }
    let mut damaged = ciphertext;
    let middle = damaged.len() / 2;
    damaged[middle] ^= 1;
    fixture
        .peer
        .rows
        .lock()
        .unwrap()
        .get_mut(&record.id)
        .unwrap()
        .blob = STANDARD.encode(damaged);
    assert!(matches!(
        fixture.store().connection(&record.id).await,
        Err(Error::Unavailable)
    ));
}

#[test]
fn deterministic_reservations_separate_domains_and_tuple_boundaries() {
    let cases: &[(&str, &[&str])] = &[
        ("pending", &["manager-a", "actor-a"]),
        ("pending", &["manager-a", "actor-b"]),
        ("pending", &["manager-b", "actor-a"]),
        ("platform-bot", &["manager-a", "actor-a"]),
        ("pending", &["ab", "c"]),
        ("pending", &["a", "bc"]),
        ("pending", &["a", "", "bc"]),
        ("pending", &["a", "bc", ""]),
        ("pending", &["a:b", "c"]),
        ("pending", &["a", "b:c"]),
    ];
    let mut observed = BTreeSet::new();
    for (domain, parts) in cases {
        let id = index_id(domain, parts);
        assert_eq!(id, index_id(domain, parts));
        assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
        assert!(observed.insert(id));
    }
}

#[tokio::test]
async fn simultaneous_create_and_cas_updates_have_one_durable_winner() {
    let fixture = Fixture::new().await;
    let first = fixture.store();
    let second = fixture.store();
    let mut left = connection("alice", false);
    let mut right = left.clone();
    left.suggested_name = Some("Left".into());
    right.suggested_name = Some("Right".into());
    let (a, b) = tokio::join!(
        first.save_connection(&left, None),
        second.save_connection(&right, None)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(Error::Conflict)) || matches!(b, Err(Error::Conflict)));
    let (mut left, version) = first.connection(&left.id).await.unwrap();
    let (mut right, other_version) = second.connection(&left.id).await.unwrap();
    assert_eq!(version, other_version);
    left.status = Status::Cancelled;
    left.browser_nonce = None;
    right.status = Status::AwaitingPlatform;
    let (a, b) = tokio::join!(
        first.save_connection(&left, Some(version)),
        second.save_connection(&right, Some(version))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let winner = if a.is_ok() { left } else { right };
    drop(first);
    drop(second);
    let (restored, next_version) = fixture.store().connection(&winner.id).await.unwrap();
    assert_eq!(next_version, version + 1);
    assert_eq!(restored.status, winner.status);
    assert_eq!(restored.browser_nonce, winner.browser_nonce);
    assert_eq!(restored.material.unwrap().token, CHILD_TOKEN);
}

#[tokio::test]
async fn committed_write_with_lost_response_is_recoverable_after_reopen() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture.peer.lose_next_put.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.store().save_connection(&record, None).await,
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        fixture.store().save_connection(&record, None).await,
        Err(Error::Conflict)
    ));
    let (stored, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(stored.fingerprint, record.fingerprint);
    assert_eq!(stored.material.unwrap().token, CHILD_TOKEN);
    assert_eq!(fixture.peer.rows.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn owner_and_organization_are_both_required_for_read_list_and_cancel() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut personal = connection("alice", false);
    personal.status = Status::Ready;
    let mut organization = connection("alice", true);
    organization.status = Status::Ready;
    let mut other = connection("bob", false);
    other.status = Status::Ready;
    let draft = connection("alice", false);
    for record in [&personal, &organization, &other, &draft] {
        fixture.store().save_connection(record, None).await.unwrap();
    }
    for (caller, id) in [
        (owner("bob", false), &personal.id),
        (owner("alice", true), &personal.id),
        (owner("alice", false), &organization.id),
    ] {
        assert!(matches!(
            service.get(&caller, id).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            service.cancel(&caller, id).await,
            Err(Error::NotFound)
        ));
    }
    let (listed, _) = service.list(&owner("alice", false), None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].connection_ref, personal.id);
    let (listed, _) = service.list(&owner("alice", true), None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].connection_ref, organization.id);
    let (still_ready, _) = fixture.store().connection(&personal.id).await.unwrap();
    assert_eq!(still_ready.status, Status::Ready);
}

#[tokio::test]
async fn platform_bot_claim_is_exclusive_but_exact_operation_recovery_is_idempotent() {
    let fixture = Fixture::new().await;
    let first = fixture.store();
    let second = fixture.store();
    let alice = uuid::Uuid::new_v4().to_string();
    let bob = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        first.claim_bot("9901", "alice", &alice),
        second.claim_bot("9901", "bob", &bob)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (subject, id) = if a.is_ok() {
        ("alice", alice)
    } else {
        ("bob", bob)
    };
    fixture
        .store()
        .claim_bot("9901", subject, &id)
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .claim_bot("9901", subject, &uuid::Uuid::new_v4().to_string())
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        fixture.store().claim_bot("9901", "other-owner", &id).await,
        Err(Error::Conflict)
    ));
    let claim_id = index_id("platform-bot", &["telegram", "9901"]);
    assert_eq!(fixture.peer.rows.lock().unwrap()[&claim_id].version, 1);
}

#[tokio::test]
async fn setup_capabilities_reject_wrong_purpose_expiry_tampering_and_other_keyring() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = connection("alice", false);
    let token = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    let opened = service.open_capability(&token, "start").unwrap();
    assert_eq!(opened.record_id, record.id);
    assert_eq!(opened.nonce, record.setup_nonce);
    assert!(matches!(
        service.open_capability(&token, "browser"),
        Err(Error::NotFound)
    ));
    let mut other_service = fixture.service();
    other_service.management.master_keys = keys(23);
    assert!(matches!(
        other_service.open_capability(&token, "start"),
        Err(Error::NotFound)
    ));
    let mut tampered = token.into_bytes();
    let index = tampered.len() / 2;
    tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
    assert!(matches!(
        service.open_capability(std::str::from_utf8(&tampered).unwrap(), "start"),
        Err(Error::NotFound)
    ));
    record.expires_at = now() - 1;
    let expired = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    assert!(matches!(
        service.open_capability(&expired, "start"),
        Err(Error::NotFound)
    ));
    assert_eq!(service.view(&record).unwrap().status, Status::Expired);
    assert!(service.view(&record).unwrap().setup_url.is_none());
}

#[tokio::test]
async fn cancelling_revokes_browser_nonce_and_setup_link_but_cannot_unlink_active_channel() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    service
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    assert_eq!(fixture.store().connection(&record.id).await.unwrap().1, 2);
    service
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    let (cancelled, version) = fixture.store().connection(&record.id).await.unwrap();
    assert!(version >= 2);
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.browser_nonce.is_none());
    assert!(service
        .get(&owner("alice", false), &record.id)
        .await
        .unwrap()
        .setup_url
        .is_none());
    let mut linked = connection("alice", false);
    linked.status = Status::Ready;
    linked.channel_record = Some(uuid::Uuid::new_v4().to_string());
    retain_bound_channel(&fixture, &linked, Some(&linked.id));
    fixture
        .store()
        .save_connection(&linked, None)
        .await
        .unwrap();
    assert!(matches!(
        service.cancel(&owner("alice", false), &linked.id).await,
        Err(Error::Conflict)
    ));
    let (unchanged, version) = fixture.store().connection(&linked.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(unchanged.status, Status::Ready);
    assert_eq!(unchanged.channel_record, linked.channel_record);
}
