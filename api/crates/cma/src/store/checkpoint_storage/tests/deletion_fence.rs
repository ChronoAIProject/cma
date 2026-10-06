use super::*;

#[tokio::test]
async fn permanent_deletion_fences_every_part_and_rejects_late_uploads() {
    let (store, fixture, server) = fixture().await;
    fixture.fenced_deletes.store(true, Ordering::SeqCst);
    let key = checkpoint_object_key("owned.cma".into(), 33_554_433);
    assert!(store.finalize_delete(&key).await.unwrap());
    let calls = fixture.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(method, _)| method == "DELETE"));
    assert_eq!(calls.last(), Some(&("DELETE".into(), key.clone())));
    for key in [
        &key,
        &multipart::part_key(&key, 0),
        &multipart::part_key(&key, 1),
    ] {
        let response = store
            .http
            .post(store.objects.clone())
            .query(&[
                ("key", key.as_str()),
                ("contentType", "application/vnd.chrono.cma-checkpoint.v1"),
            ])
            .bearer_auth(store.tokens.token().await.unwrap())
            .header("content-type", "application/octet-stream")
            .body("late bytes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    assert!(fixture.objects.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn old_or_invalid_storage_acknowledgements_cannot_end_cleanup() {
    let (store, fixture, server) = fixture().await;
    // A server ignoring mutationGeneration must not silently weaken authority.
    assert!(store.finalize_delete("owned.cma").await.is_err());
    for body in [
        br#"{"data":{"deleted":true},"error":null}"#.to_vec(),
        br#"{"data":{"deleted":true,"generation":1,"fenceVersion":1},"error":null}"#.to_vec(),
        br#"{"data":{"deleted":true,"generation":2,"fenceVersion":2},"error":null}"#.to_vec(),
        br#"{"data":{"deleted":true,"generation":2,"fenceVersion":1},"error":{"code":"failure"}}"#
            .to_vec(),
        vec![b' '; 16 * 1024 + 1],
    ] {
        let mut fault = Fault::status(StatusCode::OK);
        fault.body = Some(body);
        fixture
            .faults
            .lock()
            .unwrap()
            .entry(("DELETE".into(), "owned.cma".into()))
            .or_default()
            .push_back(fault);
        assert!(store.finalize_delete("owned.cma").await.is_err());
    }
    server.abort();
}

#[tokio::test]
async fn partial_deletion_failure_retains_work_and_replays_safely() {
    let (store, fixture, server) = fixture().await;
    fixture.fenced_deletes.store(true, Ordering::SeqCst);
    let key = checkpoint_object_key("owned.cma".into(), 33_554_433);
    fixture
        .faults
        .lock()
        .unwrap()
        .entry(("DELETE".into(), multipart::part_key(&key, 1)))
        .or_default()
        .push_back(Fault::status(StatusCode::SERVICE_UNAVAILABLE));
    assert!(store.finalize_delete(&key).await.is_err());
    assert!(!fixture.deleted_keys.lock().unwrap().contains(&key));
    assert!(store.finalize_delete(&key).await.unwrap());
    assert_eq!(fixture.deleted_keys.lock().unwrap().len(), 3);
    server.abort();
}
