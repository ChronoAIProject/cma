use super::*;

fn retained_archive(fixture: &Fixture, full_parts: usize) -> (String, String, u64) {
    let size = (full_parts * multipart::PART_BYTES + 4096) as u64;
    let key = checkpoint_object_key("cma/checkpoints/download.cma".into(), size);
    let mut objects = fixture.objects.lock().unwrap();
    let mut digest = Sha256::new();
    let mut parts = Vec::new();
    for index in 0..=full_parts {
        let bytes = vec![
            index as u8;
            if index == full_parts {
                4096
            } else {
                multipart::PART_BYTES
            }
        ];
        digest.update(&bytes);
        parts.push(format!("{:x}", Sha256::digest(&bytes)));
        objects.insert(multipart::part_key(&key, index as u64), bytes);
    }
    let sha = format!("{:x}", digest.finalize());
    objects.insert(
        key.clone(),
        serde_json::to_vec(&serde_json::json!({
            "schema": "cma-checkpoint-parts-v1",
            "size_bytes": size,
            "sha256": sha,
            "parts": parts,
        }))
        .unwrap(),
    );
    (key, sha, size)
}

fn gate_parts(fixture: &Fixture, key: &str, count: usize) -> Vec<Arc<Semaphore>> {
    (0..count)
        .map(|index| {
            let gate = Arc::new(Semaphore::new(0));
            fixture
                .read_gates
                .lock()
                .unwrap()
                .insert(multipart::part_key(key, index as u64), gate.clone());
            gate
        })
        .collect()
}

fn part_reads(fixture: &Fixture, key: &str) -> usize {
    fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, target)| method == "GET" && target.starts_with(&format!("{key}/")))
        .count()
}

#[tokio::test]
async fn download_bounds_parallel_parts_and_preserves_order_and_chunk_size() {
    let (store, fixture, server) = fixture().await;
    let full_parts = multipart::DOWNLOAD_CONCURRENCY + 1;
    let (key, sha, size) = retained_archive(&fixture, full_parts);
    let gates = gate_parts(&fixture, &key, full_parts + 1);
    let mut download = store.download(&key, "archive", &sha, size).await.unwrap();
    wait_for_part_reads(&fixture, &key, multipart::DOWNLOAD_CONCURRENCY).await;
    assert_eq!(part_reads(&fixture, &key), multipart::DOWNLOAD_CONCURRENCY);
    for gate in gates[1..multipart::DOWNLOAD_CONCURRENCY].iter().rev() {
        gate.add_permits(1);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), download.chunks.recv())
            .await
            .is_err()
    );
    assert_eq!(part_reads(&fixture, &key), multipart::DOWNLOAD_CONCURRENCY);
    gates[0].add_permits(1);
    for gate in &gates[multipart::DOWNLOAD_CONCURRENCY..] {
        gate.add_permits(1);
    }
    let mut offset = 0;
    let mut digest = Sha256::new();
    while let Some(chunk) = download.chunks.recv().await {
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= transfer::CHUNK_BYTES);
        assert!(chunk
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == ((offset + index) / multipart::PART_BYTES) as u8));
        offset += chunk.len();
        digest.update(chunk);
    }
    assert_eq!(offset as u64, size);
    assert_eq!(format!("{:x}", digest.finalize()), sha);
    assert_eq!(part_reads(&fixture, &key), full_parts + 1);
    assert_eq!(fixture.tokens.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn download_backpressure_and_cancellation_never_start_another_window() {
    let (store, fixture, server) = fixture().await;
    let (key, sha, size) = retained_archive(&fixture, multipart::DOWNLOAD_CONCURRENCY + 1);
    let mut download = store.download(&key, "archive", &sha, size).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(30), download.chunks.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.len(), transfer::CHUNK_BYTES);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(part_reads(&fixture, &key), multipart::DOWNLOAD_CONCURRENCY);
    drop(download);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(part_reads(&fixture, &key), multipart::DOWNLOAD_CONCURRENCY);
    server.abort();
}

#[tokio::test]
async fn download_cancellation_during_headers_drops_the_pending_window() {
    let (store, fixture, server) = fixture().await;
    let (key, sha, size) = retained_archive(&fixture, multipart::DOWNLOAD_CONCURRENCY + 1);
    let gates = gate_parts(&fixture, &key, multipart::DOWNLOAD_CONCURRENCY + 2);
    let download = store.download(&key, "archive", &sha, size).await.unwrap();
    wait_for_part_reads(&fixture, &key, multipart::DOWNLOAD_CONCURRENCY).await;
    drop(download);
    for gate in gates {
        gate.add_permits(1);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(part_reads(&fixture, &key), multipart::DOWNLOAD_CONCURRENCY);
    server.abort();
}

#[tokio::test]
async fn download_rejects_corrupt_short_long_and_interrupted_parts_before_delivery() {
    let (store, fixture, server) = fixture().await;
    let (key, sha, size) = retained_archive(&fixture, 1);
    for (body_size, disconnect) in [(4096, false), (4095, false), (4097, false), (0, true)] {
        fixture.faults.lock().unwrap().insert(
            ("GET".into(), multipart::part_key(&key, 1)),
            VecDeque::from([Fault {
                chunked: true,
                body: Some(vec![0x75; body_size]),
                disconnect,
                ..Fault::status(StatusCode::OK)
            }]),
        );
        let mut download = store.download(&key, "archive", &sha, size).await.unwrap();
        let mut delivered = 0;
        while let Ok(bytes) = download
            .chunks
            .recv()
            .await
            .expect("error must be delivered")
        {
            delivered += bytes.len();
        }
        assert_eq!(delivered, multipart::PART_BYTES);
        assert!(download.chunks.recv().await.is_none());
    }
    server.abort();
}

#[tokio::test]
async fn download_verifies_the_whole_archive_after_validating_each_part() {
    let (store, fixture, server) = fixture().await;
    let (key, _, size) = retained_archive(&fixture, 1);
    let wrong_sha = "0".repeat(64);
    {
        let mut objects = fixture.objects.lock().unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&objects[&key]).unwrap();
        manifest["sha256"] = wrong_sha.clone().into();
        objects.insert(key.clone(), serde_json::to_vec(&manifest).unwrap());
    }
    let download = store
        .download(&key, "archive", &wrong_sha, size)
        .await
        .unwrap();
    assert!(consume(download).await.is_err());
    server.abort();
}
