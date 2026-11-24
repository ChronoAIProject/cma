#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_upload_roundtrip_unknown_receipt_and_separate_response_replay() {
    let f = Fixture::new("media_roundtrip", true).await;
    let upload = || {
        Request::post(format!(
            "{}/attachment?name=original.bin&sha256={}&event_expires_at={}",
            f.base(),
            f.artifact.source_sha256,
            f.expiry
        ))
        .header("content-type", "application/octet-stream")
        .header("content-length", f.bytes.len())
        .body(Body::from(f.bytes.clone()))
        .unwrap()
    };
    assert_eq!(f.http(upload()).await.0, StatusCode::SERVICE_UNAVAILABLE);
    let (status, receipt) = f
        .http(
            Request::get(format!("{}/attachment", f.base()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let receipt: AgentAttachment = serde_json::from_value(receipt).unwrap();
    assert_eq!(receipt.agent_id, f.agent.id);
    assert_eq!(
        tokio::fs::read(f.files.root.join(receipt.path.trim_start_matches('/')))
            .await
            .unwrap(),
        f.bytes
    );
    assert_eq!(f.http(upload()).await.0, StatusCode::CREATED);
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 1);
    let record = f
        .store
        .get_media_upload(&receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.binding_generation, 3);
    assert_eq!(record.source_generation, None);
    let body = f.supplement(&receipt, " exact derived text\n");
    let command = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), body.clone())
        .await
        .unwrap();
    let duplicate = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), body)
        .await
        .unwrap();
    assert_eq!(command.command, duplicate.command);
    assert_eq!(command.metadata, duplicate.metadata);
    assert_ne!(
        command.command.receipt.response_id,
        f.parent.receipt.response_id
    );
    let reads = AgentReadService::new(f.store.clone(), Arc::new(Resources))
        .with_resource_access(f.access.clone());
    let authorization = reads
        .authorize_event_stream(
            &f.caller.context,
            Some(f.caller.credential.as_ref().unwrap().value()),
            &f.agent.id,
        )
        .await
        .unwrap();
    let replay = f
        .commands
        .response_for_stream(
            &authorization,
            command.command.receipt.response_id.as_ref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.metadata, command.metadata);
    let original_replay = f
        .commands
        .response_for_stream(
            &authorization,
            f.parent.receipt.response_id.as_ref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        original_replay.metadata,
        f.parent.receipt.envelope.response_metadata()
    );
    assert_eq!(
        original_replay.metadata["ordinary_private_metadata"],
        "retained original value"
    );
    assert_eq!(
        replay.metadata["cma_parent_response_id"],
        f.parent.receipt.response_id.as_ref().unwrap().to_string()
    );
    assert_eq!(
        f.store
            .get_command(&f.parent.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        f.parent
    );
    let changed = f.supplement(&receipt, "changed transcript");
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), changed)
            .await,
        Err(BindingMediaError::Conflict)
    ));
    let mut wrong = f.params();
    wrong.sha256 = sha256(b"other");
    assert!(f
        .service
        .clone()
        .prepare_upload(
            f.caller.clone(),
            f.path.clone(),
            wrong,
            f.artifact.media_type.clone(),
            f.bytes.len() as u64
        )
        .await
        .is_err());
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    assert_eq!(
        reopened
            .get_media_upload(&receipt.id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        Some(receipt)
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_pending_wrong_parent_and_revoked_source_cannot_admit() {
    let f = Fixture::new("media_refusals", false).await;
    let pending = f
        .service
        .clone()
        .prepare_upload(
            f.caller.clone(),
            f.path.clone(),
            f.params(),
            f.artifact.media_type.clone(),
            f.bytes.len() as u64,
        )
        .await
        .unwrap();
    let record = f
        .store
        .get_media_upload(&MediaUploadRecord::id(
            &f.parent.receipt.envelope.command_id,
            0,
        ))
        .await
        .unwrap()
        .unwrap();
    let body = f.supplement(&record.expected_receipt(), "");
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), body.clone())
            .await,
        Err(BindingMediaError::NotReady)
    ));
    assert!(matches!(
        f.service.attachment(f.caller.clone(), f.path.clone()).await,
        Err(BindingMediaError::NotFound)
    ));
    assert!(matches!(
        pending.upload(b"changed".to_vec()).await,
        Err(BindingMediaError::Conflict)
    ));
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 0);
    let receipt = f.upload().await.unwrap();
    let mut wrong = f.path.clone();
    wrong.event_id = "other-event".into();
    assert!(matches!(
        f.service.attachment(f.caller.clone(), wrong).await,
        Err(BindingMediaError::Forbidden)
    ));
    f.peer.active.store(false, Ordering::SeqCst);
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), body)
            .await,
        Err(BindingMediaError::Forbidden)
    ));
    assert!(matches!(
        f.service.attachment(f.caller.clone(), f.path.clone()).await,
        Err(BindingMediaError::Forbidden)
    ));
    assert_eq!(
        f.store
            .get_media_upload(&receipt.id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        Some(receipt)
    );
    assert_eq!(
        f.store
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}
