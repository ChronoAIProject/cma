use super::*;

#[tokio::test]
async fn part_deletion_is_bounded_and_verifies_every_part_before_manifest_removal() {
    let (store, fixture, server) = fixture().await;
    let size = multipart::PART_BYTES as u64 * 4 + 1;
    let key = checkpoint_object_key("cma/checkpoints/bounded-delete.cma".into(), size);
    fixture.objects.lock().unwrap().insert(key.clone(), vec![]);
    let gates: Vec<_> = (0..5).map(|_| Arc::new(Semaphore::new(0))).collect();
    for (index, gate) in gates.iter().enumerate() {
        let part = multipart::part_key(&key, index as u64);
        fixture.objects.lock().unwrap().insert(part.clone(), vec![]);
        fixture
            .read_gates
            .lock()
            .unwrap()
            .insert(part, gate.clone());
    }
    let pending_key = key.clone();
    let deletion = tokio::spawn(async move { store.delete(&pending_key).await });
    wait_for_part_reads(&fixture, &key, 4).await;
    {
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(method, _)| method == "DELETE")
                .count(),
            4
        );
        assert!(!calls.iter().any(|(_, object)| object == &key));
    }
    for gate in &gates[..4] {
        gate.add_permits(1);
    }
    wait_for_part_reads(&fixture, &key, 5).await;
    assert!(fixture.objects.lock().unwrap().contains_key(&key));
    assert!(!deletion.is_finished());
    gates[4].add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), deletion)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(fixture.objects.lock().unwrap().is_empty());
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls[calls.len() - 2], ("DELETE".into(), key.clone()));
    assert_eq!(calls[calls.len() - 1], ("GET".into(), key));
    server.abort();
}
