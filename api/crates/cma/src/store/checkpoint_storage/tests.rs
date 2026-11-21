use super::*;

#[tokio::test]
async fn retired_storage_refuses_every_blob_effect_without_reading_upload_bytes() {
    let store = RetiredCheckpointStorage;
    // Keep the producer open: retirement must not wait for or consume the archive.
    let (sender, chunks) = tokio::sync::mpsc::channel(1);
    let upload = CheckpointArchiveUpload {
        object_reference: "historical.cma".into(),
        sha256: "a".repeat(64),
        size_bytes: 1,
        chunks,
    };
    let result = tokio::time::timeout(Duration::from_secs(1), store.upload("key", upload))
        .await
        .unwrap();
    assert!(matches!(result, Err(AdapterError::Unsupported(_))));
    assert!(sender.is_closed());
    assert!(matches!(
        store
            .download("key", "historical.cma", &"a".repeat(64), 1)
            .await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(matches!(
        store.delete("key").await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(matches!(
        store.finalize_delete("key").await,
        Err(AdapterError::Unsupported(_))
    ));
}

#[test]
fn historical_object_keys_keep_their_canonical_validation() {
    let base = "cma/checkpoints/historical.cma";
    for size in [
        0,
        1,
        PART_BYTES,
        PART_BYTES + 1,
        MAX_CHECKPOINT_ARCHIVE_BYTES,
    ] {
        let key = checkpoint_object_key(base.into(), size);
        assert_eq!(checkpoint_object_key_base(&key).unwrap(), base);
        assert_eq!(
            size_from_key(&key).unwrap(),
            (size > PART_BYTES).then_some(size)
        );
    }
    for size in [
        "0".into(),
        PART_BYTES.to_string(),
        format!("0{}", PART_BYTES + 1),
        format!("+{}", PART_BYTES + 1),
        (MAX_CHECKPOINT_ARCHIVE_BYTES + 1).to_string(),
        "18446744073709551616".into(),
        "invalid".into(),
        "".into(),
    ] {
        assert!(checkpoint_object_key_base(&format!("{base}.parts-v1-{size}")).is_err());
    }
}
