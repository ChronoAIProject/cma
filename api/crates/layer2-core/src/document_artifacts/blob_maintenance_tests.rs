use super::{memory::MemoryDocumentBlobs, *};

#[tokio::test]
async fn memory_blob_maintenance_deletes_idempotently_and_allows_rewrite() {
    let blobs = MemoryDocumentBlobs::default();
    let prefix = format!("documents/{}/doc_{}/", digest(b"owner"), "a".repeat(32));
    let key = format!("{prefix}{}.md", digest(b"source"));
    assert!(blobs.deletion_supported());
    blobs.put(&key, b"source".to_vec()).await.unwrap();
    assert_eq!(
        blobs.list(&prefix, None).await.unwrap().keys.as_slice(),
        std::slice::from_ref(&key)
    );
    assert_eq!(blobs.delete(&key).await.unwrap(), BlobDeletion::Deleted);
    assert_eq!(blobs.delete(&key).await.unwrap(), BlobDeletion::Absent);
    assert_eq!(blobs.get(&key).await, Err(DocumentError::Unavailable));
    blobs.put(&key, b"source".to_vec()).await.unwrap();
    assert_eq!(blobs.get(&key).await.unwrap(), b"source");
}

#[tokio::test]
async fn memory_blob_maintenance_pages_stably_and_refuses_foreign_prefixes() {
    let blobs = MemoryDocumentBlobs::default();
    let prefix = format!("documents/{}/", digest(b"owner"));
    let foreign = format!("documents/{}/", digest(b"other"));
    for index in 0..257 {
        let key = format!("{prefix}doc_{}/{index:064x}.md", "b".repeat(32));
        blobs.put(&key, b"source".to_vec()).await.unwrap();
    }
    blobs
        .put(&format!("{foreign}ignored"), vec![1])
        .await
        .unwrap();
    let first = blobs.list(&prefix, None).await.unwrap();
    assert_eq!(first.keys.len(), 256);
    let last = blobs.list(&prefix, first.next.as_deref()).await.unwrap();
    assert_eq!(last.keys.len(), 1);
    assert!(last.next.is_none());
    assert!(first.keys.last().unwrap() < &last.keys[0]);
    for bad in [
        "documents/",
        "https://other.invalid/",
        "cma/checkpoints/",
        "../",
    ] {
        assert_eq!(
            blobs.list(bad, None).await.unwrap_err().error,
            DocumentError::Invalid
        );
        assert_eq!(
            blobs.delete(bad).await.unwrap_err().error,
            DocumentError::Invalid
        );
    }
}
